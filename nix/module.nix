{
  config,
  lib,
  ...
}:

let
  cfg = config.services.tapas;
  database = "/var/lib/tapas/tapas.db";
in
{
  options.services.tapas = {
    enable = lib.mkEnableOption "the tapas server";

    package = lib.mkOption { type = lib.types.package; };

    domain = lib.mkOption {
      type = lib.types.str;
      description = "Public host name; Caddy gets its certificate from Let's Encrypt.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8080;
    };

    allowedEmail = lib.mkOption {
      type = lib.types.str;
      description = "The only Google account allowed to sign in.";
    };

    googleWebClientId = lib.mkOption {
      type = lib.types.str;
      description = "Client id of the Google Cloud *Web application* OAuth client.";
    };

    secretsDir = lib.mkOption {
      type = lib.types.path;
      default = "/etc/tapas";
      description = ''
        Root-only directory holding `google-web-client-secret`, `token-key` (32 random bytes)
        and, with a backup replica, `litestream.env`. Copied once by `nixos-anywhere --extra-files`.
      '';
    };

    backupReplica = lib.mkOption {
      type = lib.types.nullOr lib.types.attrs;
      default = null;
      description = "Litestream replica settings for the database, e.g. an S3 bucket.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.tapas = {
      isSystemUser = true;
      group = "tapas";
    };
    users.groups.tapas = { };

    systemd.services.tapas = {
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      environment = {
        TAPAS_LISTEN = "127.0.0.1:${toString cfg.port}";
        TAPAS_DATABASE = database;
        TAPAS_ALLOWED_EMAIL = cfg.allowedEmail;
        TAPAS_GOOGLE_WEB_CLIENT_ID = cfg.googleWebClientId;
        TAPAS_GOOGLE_WEB_CLIENT_SECRET_FILE = "%d/google-web-client-secret";
        TAPAS_TOKEN_KEY_FILE = "%d/token-key";
        RUST_LOG = "info";
      };
      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} serve";
        User = "tapas";
        Group = "tapas";
        LoadCredential = [
          "google-web-client-secret:${cfg.secretsDir}/google-web-client-secret"
          "token-key:${cfg.secretsDir}/token-key"
        ];
        StateDirectory = "tapas";
        # Litestream reads and writes the WAL next to the database through the group.
        StateDirectoryMode = "0770";
        UMask = "0007";
        Restart = "on-failure";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
      };
    };

    services.caddy = {
      enable = true;
      virtualHosts.${cfg.domain}.extraConfig = ''
        reverse_proxy 127.0.0.1:${toString cfg.port}
      '';
    };
    networking.firewall.allowedTCPPorts = [
      80
      443
    ];

    services.litestream = lib.mkIf (cfg.backupReplica != null) {
      enable = true;
      environmentFile = "${cfg.secretsDir}/litestream.env";
      settings.dbs = [
        {
          path = database;
          replicas = [ cfg.backupReplica ];
        }
      ];
    };
    users.users.litestream.extraGroups = lib.mkIf (cfg.backupReplica != null) [ "tapas" ];
  };
}
