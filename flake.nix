{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    {
      self,
      flake-utils,
      naersk,
      nixpkgs,
    }:
    {
      # Top-level, system-independent overlay
      overlays.default =
        final: _:
        let
          naersk' = final.callPackage naersk { };
        in
        {
          mash = naersk'.buildPackage {
            buildInputs = with final; [
              perl
              openssl
            ];
            nativeBuildInputs = with final; [
              perl
              openssl
            ];
            src = self;
          };
        };
    }
    // flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ self.overlays.default ];
        };
      in
      {
        # For `nix build` & `nix run`:
        defaultPackage = pkgs.mash;

        # For `nix develop`:
        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo-audit
            cargo-nextest
            grcov
            llvmPackages_19.libllvm
            rust-analyzer
          ];
        };
      }
    );
}
