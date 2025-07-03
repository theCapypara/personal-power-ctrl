{
  description = "personal-power-ctrl";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    flake-utils.url = "github:numtide/flake-utils";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    {
      self,
      nixpkgs,
      systems,
      flake-utils,
    }:
    flake-utils.lib.eachSystem (import systems) (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages = rec {
          personal-power-ctrl = pkgs.callPackage ./nix/pkg.nix { };
          default = personal-power-ctrl;
        };

        devShells = {
          default = pkgs.callPackage ./nix/shell.nix { };
        };
      }
    );
}
