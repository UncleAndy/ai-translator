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
        isLinux = pkgs.stdenv.isLinux;
        # PipeWire, ALSA and glibc are only needed to build on Linux: cpal pulls
        # them in through its pipewire feature, libspa-sys wants the PipeWire
        # headers, and bindgen wants glibc's. On Darwin the audio backend is
        # CoreAudio, a system framework, and none of the three exist there.
        linuxOnly = pkgs.lib.optionals isLinux;
      in {
        devShells.default = pkgs.mkShell {
          name = "ai-translator-dev";

          shellHook = ''
            mkdir -p .cargo
            cat <<EOF > .cargo/config.toml
[env]
PKG_CONFIG_PATH = { value = "${pkgs.lib.makeSearchPath "lib/pkgconfig" (linuxOnly [ pkgs.pipewire.dev pkgs.alsa-lib.dev ] ++ [ pkgs.openssl.dev ])}", relative = false }
LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib"
EOF
            ${pkgs.lib.optionalString isLinux ''
              echo 'BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include"' >> .cargo/config.toml
            ''}
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

          buildInputs = with pkgs; linuxOnly [
            glibc.dev
            alsa-lib
            pipewire
          ] ++ [
            openssl
            llvmPackages.libclang
          ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (linuxOnly [
            pkgs.alsa-lib
            pkgs.pipewire
          ] ++ [
            pkgs.openssl
          ]);
        };
      });
}
