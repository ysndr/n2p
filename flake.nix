{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };
  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        n2p = pkgs.callPackage ./packages/n2p/package.nix { inherit self; };
      in
      {
        packages = {
          inherit n2p;
          default = n2p;
        };
        apps = {
          n2p = flake-utils.lib.mkApp { drv = self.packages.${system}.n2p; };
          default = n2p;
        };
        devShells = {
          default = pkgs.mkShell {
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            inputsFrom = [ n2p ];
            packages = [ pkgs.clippy pkgs.rustfmt ];
          };
        };
      }
    );
}
