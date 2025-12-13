{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-25.11";
    nixpkgs-unstable.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs =
    inputs@{ self, nixpkgs, ... }:
    let
      system = "x86_64-linux";
      unstablePkgs = import inputs.nixpkgs-unstable { inherit system; };
    in
    {
      devShells."${system}".default =
        let
          pkgs = import nixpkgs { inherit system; };
          dependencies = with pkgs; [
            kmod
            llvmPackages.libclang
            pkg-config
          ];
        in
        pkgs.mkShell {
          pure = true;
          nativeBuildInputs = dependencies;

          packages = [
            pkgs.rustup
            unstablePkgs.nushell
          ];

          env = {
            LIBRARY_PATH = "${pkgs.lib.makeLibraryPath dependencies}";
            LD_LIBRARY_PATH = "${pkgs.lib.makeLibraryPath dependencies}";
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            BINDGEN_EXTRA_CLANG_ARGS = "-I${pkgs.lib.getDev pkgs.kmod}/include -I${pkgs.lib.getDev pkgs.glibc}/include";
            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          };

          # shellHook = ''
          #   exec nu
          # '';
        };

    };
}
