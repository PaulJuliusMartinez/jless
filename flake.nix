# Nix flake for jless. This means that where Nix is installed (see
# https://nixos.org/explore.html), jless can be run immediately via
#
#   nix run github:PaulJuliusMartinez/jless
#
# or with arguments like this:
#
#   nix run github:PaulJuliusMartinez/jless -- --help
#
# and is easy to include in a machine or user environment via the overlay.
{
  description = "A command-line pager for JSON data";

  inputs.utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, utils }:
    let
      # Get metadata from Cargo.toml
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      name = cargoToml.package.name;

      # How to build
      build = { rustPlatform }: rustPlatform.buildRustPackage
        {
          pname = name;
          version = cargoToml.package.version;
          src = ./.;
          cargoLock = { lockFile = ./Cargo.lock; };
          doCheck = true;
        };

      # Create overlay that includes this package
      overlay = final: prev: {
        ${name} = final.callPackage build { };
      };
      overlays = [ overlay ];
    in

    # Finally, define the flake outputs
    { inherit overlay overlays; } //
    (utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system overlays; };
      in
      rec {
        # nix build
        packages.${name} = pkgs.${name};
        defaultPackage = packages.${name};

        # nix develop
        devShell = pkgs.mkShell {
          inputsFrom = [ defaultPackage ];
          buildInputs = with pkgs; [ rustc rust-analyzer clippy rustfmt ];
        };
      }));
}
