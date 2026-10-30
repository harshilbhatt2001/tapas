# The Hetzner Cloud VPS (x86, CX22 class). Installed with nixos-anywhere; see README.
{ modulesPath, ... }:

{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
    ./disko.nix
  ];

  services.tapas = {
    enable = true;
    domain = "tapas.example.com";
    allowedEmail = "harshilbhatt2001@gmail.com";
    googleWebClientId = "REPLACE-ME.apps.googleusercontent.com";
    backupReplica = {
      type = "s3";
      endpoint = "https://fsn1.your-objectstorage.com";
      region = "fsn1";
      bucket = "tapas-backup";
      path = "tapas.db";
    };
  };

  boot.loader.grub = {
    efiSupport = true;
    efiInstallAsRemovable = true;
  };

  networking.hostName = "tapas";

  services.openssh = {
    enable = true;
    settings = {
      PasswordAuthentication = false;
      KbdInteractiveAuthentication = false;
      PermitRootLogin = "prohibit-password";
    };
  };
  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILCRqAWLlgoI3AqGzTfz/LAcWAC5n43vvc6fFFlqK73K harshilbhatt2001@gmail.com"
  ];

  # Security patches: nightly rebuild from the public repo with the newest nixpkgs of the pinned
  # release. A failed build keeps the running system.
  system.autoUpgrade = {
    enable = true;
    flake = "github:harshilbhatt2001/tapas#tapas";
    flags = [
      "--update-input"
      "nixpkgs"
      "--no-write-lock-file"
    ];
    dates = "04:00";
    allowReboot = true;
  };

  nix = {
    settings.experimental-features = [
      "nix-command"
      "flakes"
    ];
    gc = {
      automatic = true;
      options = "--delete-older-than 14d";
    };
  };

  system.stateVersion = "26.05";
}
