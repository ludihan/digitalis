{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      forAllSystems = with nixpkgs; lib.genAttrs lib.systems.flakeExposed;
      nixpkgsFor = forAllSystems (system: import nixpkgs { inherit system; });
    in
    {

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgsFor.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              nodejs
              nodePackages.npm
              vtsls
              go
              gopls
              gotools
              go-tools
              sqlite-interactive
            ];
            shellHook = ''
              export PS1="(digitalis) $PS1"
            '';
          };
        }
      );

    };
}
