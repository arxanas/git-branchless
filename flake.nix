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
      foreachSystem = f: lib.genAttrs systems (system: f {
        pkgs = nixpkgs.legacyPackages.${system};
        /** final packages set (of a given system) provided in this flake */
        final = self.packages.${system};
      });
    in
    {
      overlays.default = (final: prev: {

        # reuse the definition from nixpkgs git-branchless
        git-branchless = prev.git-branchless.overrideAttrs ({ meta, ... }: {
          name = "git-branchless";
          src = self;
          patches = [ ];
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

        # reuse the definition for git-branchless
        scm-diff-editor = final.git-branchless.overrideAttrs (finalAttrs: prevAttrs: {
          name = "scm-diff-editor";
          meta = prevAttrs.meta // {
            mainProgram = finalAttrs.name;
            description = "UI to interactively select changes, bundled in git-branchless";
          };

          buildAndTestSubdir = "scm-record";
          buildFeatures = [ "scm-diff-editor" ];

          # remove the git-branchless specific build commands
          postInstall = "";
          preCheck = "";
          checkFlags = "";
        });
      });

      packages = foreachSystem ({ pkgs, ... }:
        let
          final = pkgs.extend self.overlays.default;
        in
        {
          inherit (final)
            git-branchless scm-diff-editor;
          default = final.git-branchless;
        }
      );

      devShells = foreachSystem ({ pkgs, final }: {
        default = final.git-branchless.overrideAttrs ({ nativeBuildInputs, ... }: {

          nativeBuildInputs = with pkgs.buildPackages; [
            cargo # with shell completions, instead of cargo-auditable
            git # for testing
          ] ++ nativeBuildInputs;

          env = with pkgs.buildPackages; {
            # for developments, e.g. symbol lookup in std library
            RUST_SRC_PATH = "${rustPlatform.rustLibSrc}";
            # for testing
            TEST_GIT = "${git}/bin/git";
            TEST_GIT_EXEC_PATH = "${git}/libexec/git-core";
          };
        });
      });

      checks = foreachSystem ({ pkgs, final }: {
        git-branchless =
          final.git-branchless.overrideAttrs ({ preCheck, ... }: {
            cargoBuildType = "debug";
            cargoCheckType = "debug";
            preCheck = ''
              export RUST_BACKTRACE=1
            '' + preCheck;
          });
      });
    };
}
