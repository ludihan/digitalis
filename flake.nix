{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      lib = nixpkgs.lib;
      overlays = [ (import rust-overlay) ];
      supported-systems = lib.systems.flakeExposed;
      forAllSystems =
        f:
        lib.genAttrs supported-systems (
          system:
          f rec {
            pkgs = import nixpkgs {
              config.allowUnfree = true;
              inherit system overlays;
            };
            inherit system;
          }
        );
    in
    {
      devShells = forAllSystems (
        { pkgs, ... }:
        {
          default =
            with pkgs;
            mkShell {
              buildInputs = [
                rust-analyzer
                rust-bin.stable.latest.default
                pkg-config
                alsa-lib

                vtsls
                nodejs
              ];

              shellHook = ''
                export PS1="(digitalis) $PS1"
              '';
            };
        }
      );
    };
}
