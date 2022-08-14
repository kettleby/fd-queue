{
  description = "Development Environment for fd-queue crate.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-apps.url = "github:sbosnick/rust-apps";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, rust-apps }:
    flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs {
          inherit system;
          overlays = [
            rust-overlay.overlays.default
            rust-apps.overlays.default
          ];
        };
    in
    {
      devShells.default = pkgs.mkShell {
        nativeBuildInputs = [
          pkgs.rust-bin.stable.latest.default
          pkgs.rust-apps.release-plz
          pkgs.cargo-edit
          pkgs.cargo-outdated
          pkgs.cargo-audit
        ];
     };
    });
 }
