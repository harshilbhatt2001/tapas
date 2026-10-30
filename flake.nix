{
  description = "tapas: a closed-loop roadmap tracker, and the NixOS host that runs it";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    disko = {
      url = "github:nix-community/disko";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      disko,
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system} = {
        tapas-server = pkgs.callPackage ./nix/package.nix { };
        default = self.packages.${system}.tapas-server;
      };

      nixosModules.tapas = ./nix/module.nix;

      nixosConfigurations.tapas = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          disko.nixosModules.disko
          self.nixosModules.tapas
          ./nix/host
          { services.tapas.package = self.packages.${system}.tapas-server; }
        ];
      };
    };
}
