Welcome to `git-branchless`! Please review the [code of conduct](/CODE_OF_CONDUCT.md) before participating in the project.

# Bugs and feature requests

You can report bugs in the [Github issue tracker](https://github.com/arxanas/git-branchless/issues). There is no formal issue template at this time. For bugs, please provide a [Short, Self-Contained, Correct Example](http://sscce.org/).

For feature requests, workflow questions, and other open-ended topics, [start a discussion](https://github.com/arxanas/git-branchless/discussions).

# Development

See the [Architecture document](https://github.com/arxanas/git-branchless/wiki/Architecture).

Run tests with `cargo test`. The tests depend on the version of Git you're using, so you need to provide a Git executable as an environment variable with the `PATH_TO_GIT` variable. For example:

```
$ PATH_TO_GIT=$(which git) cargo test  # use globally-installed version
$ PATH_TO_GIT=/path/to/dir/git cargo test  # use the `git` executable inside /path/to/dir
```

# Maintenance

TODO: update these instructions to use `cargo release`. See
* `cargo release` page: https://github.com/sunng87/cargo-release
* Example metadata: https://github.com/agavrilov/cursive_buffered_backend/blob/master/Cargo.toml
* Example instructions: https://github.com/agavrilov/cursive_buffered_backend/blob/master/ReleaseInstructions.md

To release a new version:

* Update the version string in `Cargo.toml`.
* Update `CHANGELOG.md` and add a new header for the about-to-be-released version. Make sure to keep an empty `[Unreleased]` section header at the top.
* Commit the above changes with a message like `docs: release version v1.2.3`.
* Tag the previous commit with the version number (`git tag v1.2.3`).
* Push the commit to Github.
* Run `cargo publish` to publish the code to `crates.io`.
* [Create a Github release](https://github.com/arxanas/git-branchless/releases/new) for the version tag. Leave the release title empty to automatically use the tag name as the release title. Copy and paste the changelog contents for this version into the release notes.
