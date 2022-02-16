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
    {
      overlay = (final: prev: {
        git-branchless = final.callPackage
          (
            { lib
            , git
            , libiconv
            , ncurses
            , openssl
            , pkg-config
            , rustPlatform
            , sqlite
            , stdenv
            , Security
            , SystemConfiguration
            }:

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
              ] ++ lib.optionals stdenv.isDarwin [
                Security
                SystemConfiguration
                libiconv
              ];

              preCheck = ''
                export TEST_GIT=${git}/bin/git
                export TEST_GIT_EXEC_PATH=$(${git}/bin/git --exec-path)
              '';
              # FIXME: these tests deadlock when run in the Nix sandbox
              checkFlags = [
                "--skip=test_checkout_pty"
                "--skip=test_next_ambiguous_interactive"
              ];
            }
          )
          {
            inherit (final.darwin.apple_sdk.frameworks) Security SystemConfiguration;
          };
      });
    } //
    (foreachSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ self.overlay ];
        };
      in
      {
        packages.${system}.git-branchless = pkgs.git-branchless;
        defaultPackage.${system} = self.packages.${system}.git-branchless;
        checks.${system}.git-branchless = pkgs.git-branchless.overrideAttrs ({ preCheck, ... }: {
          cargoBuildType = "debug";
          cargoCheckType = "debug";
          preCheck = ''
            export RUST_BACKTRACE=1
          '' + preCheck;
        });
      }));
}
