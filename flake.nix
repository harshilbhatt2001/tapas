{
  description = "Terminal training-week planner with Google Calendar and Google Health sync";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        tapas = pkgs.callPackage ./nix/package.nix { };
        default = tapas;
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          meta.description = "Run the tapas TUI";
          program = pkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.tapas;
        };
      });
    };
}
