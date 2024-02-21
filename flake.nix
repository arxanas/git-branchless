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

        # reuse the definition from nixpkgs git-branchless
        git-branchless = prev.git-branchless.overrideAttrs ({ meta, ... }: {
          name = "git-branchless";
          src = self;
          cargoDeps = final.rustPlatform.importCargoLock {
            lockFile = ./Cargo.lock;
          };

          # for `flake.nix` contributors: put additional local overrides here.
          # if the changes are also applicable to the `git-branchless` package
          # in nixpkgs, consider first improving the definition there, and then
          # update the `flake.lock` here.

          # in case local overrides might confuse upstream maintainers,
          # we do not list them here:
          meta = (removeAttrs meta [ "maintainers" ]) // {
            # to correctly generate meta.position for back trace:
            inherit (meta) description;
          };
        });

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
