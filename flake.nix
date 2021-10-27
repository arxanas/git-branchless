{
  description = "git-branchless";

  outputs = { self, nixpkgs, ... }:
    let
      lib = nixpkgs.lib;
      systems = [
        "aarch64-linux"
        "aarch64-darwin"
        "i686-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      foreachSystem = f: lib.foldl' (attrs: system: lib.recursiveUpdate attrs (f system)) { } systems;
    in
    foreachSystem (system: {
      packages.${system}.git-branchless = with nixpkgs.legacyPackages.${system};
        rustPlatform.buildRustPackage {
          name = "git-branchless";

          src = self;

          cargoLock = {
            lockFile = "${self}/Cargo.lock";
          };

          nativeBuildInputs = [ pkg-config ];

          buildInputs = [
            ncurses
            openssl
            sqlite
          ] ++ lib.optionals stdenv.isDarwin (with darwin.apple_sdk.frameworks; [
            Security
            SystemConfiguration
            libiconv
          ]);

          buildType = "debug";
          preCheck = ''
            export RUST_BACKTRACE=1
            export PATH_TO_GIT=${git}/bin/git
            export GIT_EXEC_PATH=$(${git}/bin/git --exec-path)
          '';
          # FIXME: these tests deadlock when run in the Nix sandbox
          checkFlags = [
            "--skip=test_checkout_pty"
            "--skip=test_next_ambiguous_interactive"
          ];
        };
      defaultPackage.${system} = self.packages.${system}.git-branchless;
    });
}
