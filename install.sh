#!/bin/bash
set -eo pipefail

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}git-branchless GPG fork installer${NC}"
echo "This script will install git-branchless with enhanced GPG signing support"
echo

# Check if Rust/Cargo is installed
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}Error: Cargo is not installed.${NC}"
    echo "Please install Rust and Cargo first: https://rustup.rs/"
    exit 1
fi

# Check if Git is installed
if ! command -v git &> /dev/null; then
    echo -e "${RED}Error: Git is not installed.${NC}"
    echo "Please install Git first"
    exit 1
fi

# Check if GPG is installed
if ! command -v gpg &> /dev/null; then
    echo -e "${YELLOW}Warning: GPG is not installed.${NC}"
    echo "The GPG signing features will not work without GPG installed."
    read -p "Continue anyway? [y/N] " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Installation aborted."
        exit 1
    fi
fi

echo "Building git-branchless with GPG support..."
cargo build --release

echo "Installing git-branchless..."
cargo install --path git-branchless

# Check if installation was successful
if command -v git-branchless &> /dev/null; then
    echo -e "${GREEN}git-branchless has been successfully installed!${NC}"
else
    echo -e "${YELLOW}git-branchless was built but may not be in your PATH.${NC}"
    
    # Detect shell
    SHELL_NAME=$(basename "$SHELL")
    SHELL_RC=""
    
    if [[ "$SHELL_NAME" == "bash" ]]; then
        SHELL_RC="$HOME/.bashrc"
    elif [[ "$SHELL_NAME" == "zsh" ]]; then
        SHELL_RC="$HOME/.zshrc"
    fi
    
    if [[ -n "$SHELL_RC" ]]; then
        echo "You may need to add the Cargo bin directory to your PATH:"
        echo "echo 'export PATH=\$PATH:\$HOME/.cargo/bin' >> $SHELL_RC"
        echo "source $SHELL_RC"
    else
        echo "Add the following to your shell configuration:"
        echo "export PATH=\$PATH:\$HOME/.cargo/bin"
    fi
fi

echo
echo -e "${GREEN}Installation complete!${NC}"
echo
echo "To use git-branchless in a repository:"
echo "  1. Navigate to your repository:      cd /path/to/your/repo"
echo "  2. Initialize git-branchless:        git-branchless init"
echo "  3. Use the commands:                 git branchless smartlog"
echo
echo "To configure GPG signing:"
echo "  git config --global user.signingkey YOUR_GPG_KEY_ID"
echo "  git config --global commit.gpgsign true  # Optional: sign all commits by default"
echo
echo "For more information, see the README.md" 
