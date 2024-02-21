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
      foreachSystem = lib.genAttrs systems;
    in
    {
      overlays.default = (final: prev: {
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
              # FIXME: these tests time out when run in the Nix sandbox
              checkFlags = [
                "--skip=test_switch_pty"
                "--skip=test_next_ambiguous_interactive"
                "--skip=test_switch_auto_switch_interactive"
              ];
            }
          )
          {
            inherit (final.darwin.apple_sdk.frameworks) Security SystemConfiguration;
          };

        scm-diff-editor = final.callPackage
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
              name = "scm-diff-editor";

              src = self;

              cargoLock = {
                lockFile = "${self}/Cargo.lock";
              };

              buildAndTestSubdir = "scm-record";
              buildFeatures = [ "scm-diff-editor" ];
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
            }
          )
          {
            inherit (final.darwin.apple_sdk.frameworks) Security SystemConfiguration;
          };
      });

      packages = foreachSystem (system:
        let
          pkgs = nixpkgs.legacyPackages.${system}.extend self.overlays.default;
        in
        {
          inherit (pkgs)
            git-branchless scm-diff-editor;
          default = pkgs.git-branchless;
        }
      );

      checks = foreachSystem (system: {
        git-branchless =
          self.packages.${system}.git-branchless.overrideAttrs ({ preCheck, ... }: {
            cargoBuildType = "debug";
            cargoCheckType = "debug";
            preCheck = ''
              export RUST_BACKTRACE=1
            '' + preCheck;
          });
      });
    };
}
