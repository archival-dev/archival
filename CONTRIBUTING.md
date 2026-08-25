# Contributing to archival

If you're new to open source, the [Open Source Guides](https://opensource.guide/) is a good place to learn about how things usually work. Here are some especially helpful guides for folks just getting started:

- [How to Contribute to Open Source](https://opensource.guide/how-to-contribute/)
- [Building Welcoming Communities](https://opensource.guide/building-community/)

## Helping make archival better

archival is intentionally a very small tool, but since it runs in lots of different environments (OSs, WASM, CI), and has an ecosystem of sites and hosts, there's plenty of ways it can be improved. Here are some of the ones we focus on.

- Build things with it - archival makes different choices about data, which means there are always new and interesting ways to use it. If you find your use case isn't smooth, reach out or [file an issue](#reporting-new-issues).
- Check out the [open issues](https://github.com/archival-dev/archival/issues) to see if anything looks fun to fix. We use the tag [good first issue](https://github.com/archival-dev/archival/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) to indicate ones that are good for folks who haven't contributed yet.
- Join the [discord](https://archival.dev/chat.html) and ask questions, get the latest updates, and chat with us about your projects.

### Reporting new issues

The public issue tracker is [here on github](https://github.com/archival-dev/archival/issues). For documentation issues, use the [documentation repo](https://github.com/archival-dev/archival-docs/issues).

### Installation

To work on the archival library, all you need is the rust toolchain:

https://doc.rust-lang.org/cargo/getting-started/installation.html

Then, you can clone this repo and develop it using `cargo`.

### Developing

While we're still young, just ask for help on [discord](https://archival.dev/chat.html) - at a high level, the main thing to be aware of is that archival is cross-compiled into binaries for every major OS, mobile OSes, and WASM - so it's important to run the tests and pay attention to which features your code will run under.

To assist in avoiding finding about issues before breaking in CI, add a pre-commit hook:

```bash
echo "./pre-commit.sh" > .git/hooks/pre-commit
```

### Creating a branch

Fork [the repository](https://github.com/archival-dev/archival) and create your branch from `main`. If you've never sent a GitHub pull request before, you can learn how from [this free video series](https://egghead.io/courses/how-to-contribute-to-an-open-source-project-on-github).

### Testing

archival uses a vanilla cargo setup for tests, and all tests can be run with `./test.sh` in the root of the repo.

### Releasing

archival's version is written down in several places: `Cargo.toml`, `Cargo.lock`, `package.json`, the `archival-version` default in `action.yml`, and the `$id` of each versioned schema published to SchemaStore. Set them all at once with:

```bash
./bump-version.sh 0.18.0
```

Then commit the result and tag it `v0.18.0` - CI publishes the crate, the binaries, and the npm package from the tag.

`./check-versions.sh` verifies that every one of those places agrees with `Cargo.toml` (and, given a version argument, that they match it). It runs in the pre-commit hook and again in CI against the release tag, so a version that only got half-bumped fails before it ships.

### Style guide

archival uses `clippy` with default settings as the linter. See docs:
https://doc.rust-lang.org/stable/clippy/installation.html

## License

By contributing to archival, you agree that your contributions will be licensed under its [GNU Affero General Public License v3.0 or later](https://github.com/archival-dev/archival/blob/main/LICENSE).
