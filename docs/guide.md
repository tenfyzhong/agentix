# Agentix detailed guide

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Detailed-Guide). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

## Release packaging

The Homebrew formulae for Agentix and the standalone task manager, taskix, are maintained in [tenfyzhong/homebrew-tap](https://github.com/tenfyzhong/homebrew-tap). Release automation updates both formulae and publishes macOS arm64, Linux x86_64, and Linux arm64 bottles. Each platform compiles the CLIs once and reuses those binaries for both release archives and Homebrew bottles. Bottle packaging preserves the prepared source formula and verifies a real bottle installation; after every platform succeeds, automation merges the metadata into one pull request per formula. Install taskix with `brew install tenfyzhong/tap/taskix`. The Homebrew workflow can also be run manually for an existing release tag, selecting `agentix`, `taskix`, or `all` (the default) to download and checksum-verify the existing release binaries, package the corresponding bottles, and publish formula pull requests.
