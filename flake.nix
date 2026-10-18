{
  description = "ai-translator development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = pkgs.mkShell {
          name = "ai-translator-dev";

          shellHook = ''
            mkdir -p .cargo
            cat <<EOF > .cargo/config.toml
[env]
PKG_CONFIG_PATH = { value = "${pkgs.lib.makeSearchPath "lib/pkgconfig" [ pkgs.pipewire.dev pkgs.alsa-lib.dev pkgs.openssl.dev ]}", relative = false }
LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib"
BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include"
EOF
            export CARGO_TARGET_DIR="$HOME/.cache/ai-translator-target"
          '';

          nativeBuildInputs = with pkgs; [
            pkg-config
            clang
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
          ];

          buildInputs = with pkgs; [
            glibc.dev
            alsa-lib
            pipewire
            openssl
            llvmPackages.libclang
          ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.alsa-lib
            pkgs.pipewire
            pkgs.openssl
          ];
        };
      });
}
