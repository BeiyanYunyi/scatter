{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      flake-parts,
      rust-overlay,
      crane,
      ...
    }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      perSystem =
        { system, pkgs, ... }:
        let
          craneLib = (crane.mkLib pkgs).overrideToolchain (
            p: p.rust-bin.selectLatestNightlyWith (toolchain: toolchain.minimal)
          );
          run-dll = with pkgs; [
            wayland
            vulkan-loader
            vulkan-validation-layers
            vulkan-tools
            libxkbcommon
          ];
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          devShells.default = pkgs.mkShell {
            name = "scatter-dev-shell";
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath run-dll;
          };
          packages.default =
            with pkgs;
            let
              assetFilter =
                path: _type:
                (builtins.match ".*/src/.*\\.wgsl$" path != null)
                || (builtins.match ".*/data/stars/.*" path != null);
              assetOrCargo = path: type: (assetFilter path type) || (craneLib.filterCargoSources path type);
            in
            craneLib.buildPackage {
              src = lib.cleanSourceWith {
                src = ./.;
                filter = assetOrCargo;
                name = "source";
              };
              # Add extra inputs here or any other derivation settings
              # doCheck = true;
              nativeBuildInputs = [ makeBinaryWrapper ];
              postInstall = ''
                wrapProgram $out/bin/scatter --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath run-dll}
              '';
              CI = "true";
              meta.mainProgram = "scatter";
            };
        };
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
    };
}
