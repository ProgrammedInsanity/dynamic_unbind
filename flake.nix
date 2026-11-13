{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs =
    inputs@{ nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      unstablePkgs = import inputs.nixpkgs-unstable { inherit system; };
      staticPkgs = pkgs.pkgsStatic;
      dependencies = with pkgs; [
        kmod
        llvmPackages.libclang
        pkg-config
      ];
      staticLibraries = [
        staticPkgs.kmod
        staticPkgs.xz
        staticPkgs.zstd
      ];
    in
    {
      devShells."${system}".default = pkgs.mkShell {
        nativeBuildInputs = dependencies;

        packages = [
          pkgs.rustup
          unstablePkgs.nushell
          pkgs.binutils
        ];

        env = {
          LIBRARY_PATH = "${pkgs.lib.makeLibraryPath dependencies}";
          LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath dependencies}";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";

          BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu = "-I${pkgs.lib.getDev pkgs.kmod}/include -I${pkgs.lib.getDev pkgs.glibc}/include";
          BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_musl = "-I${pkgs.lib.getDev staticPkgs.kmod}/include -I${pkgs.lib.getDev staticPkgs.stdenv.cc.libc}/include";
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${staticPkgs.stdenv.cc}/bin/${staticPkgs.stdenv.cc.targetPrefix}cc";
          # kmod-sys does not emit libkmod's static compression dependencies.
          CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS =
            pkgs.lib.concatMapStringsSep " " (p: "-L native=${pkgs.lib.getLib p}/lib") staticLibraries
            + " -C target-feature=+crt-static -C link-arg=-llzma -C link-arg=-lzstd";
        };
      };
    };
}
