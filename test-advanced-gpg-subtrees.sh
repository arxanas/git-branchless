#!/bin/bash
set -eo pipefail

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${GREEN}Testing git-branchless advanced operations with GPG signing${NC}"
echo

# Check if GPG is installed
if ! command -v gpg &> /dev/null; then
    echo -e "${RED}Error: GPG is not installed.${NC}"
    echo "Please install GPG to run this test."
    exit 1
fi

# Check if git-branchless is installed
if ! command -v git-branchless &> /dev/null; then
    echo -e "${RED}Error: git-branchless is not installed or not in PATH.${NC}"
    echo "Please install git-branchless first."
    exit 1
fi

# Create a temporary test directory
TEST_DIR=$(mktemp -d)
echo "Creating test repository in: $TEST_DIR"
cd "$TEST_DIR"

# Initialize a git repository
git init
echo "# Test Repository" > README.md
git add README.md
git commit -m "Initial commit"

# Configure GPG signing for this test repository
echo
echo -e "${YELLOW}GPG Configuration${NC}"
echo "We'll now set up GPG signing for this test repository."
echo "If you have a GPG key already, please enter the key ID."
echo "If not, press Enter and we'll try to use your default key."
echo

read -p "Enter your GPG key ID (or press Enter for default): " GPG_KEY_ID

if [ -z "$GPG_KEY_ID" ]; then
    # Try to get the default key
    GPG_KEY_ID=$(gpg --list-secret-keys --keyid-format=long | grep sec | head -n 1 | awk '{print $2}' | cut -d'/' -f2)
    
    if [ -z "$GPG_KEY_ID" ]; then
        echo -e "${RED}No GPG key found. Please create a GPG key first.${NC}"
        echo "You can create a key with: gpg --full-generate-key"
        exit 1
    fi
    
    echo "Using default GPG key: $GPG_KEY_ID"
else
    echo "Using provided GPG key: $GPG_KEY_ID"
fi

# Configure Git to use the GPG key
git config user.signingkey "$GPG_KEY_ID"
git config commit.gpgsign true
git config user.name "Test User"
git config user.email "test@example.com"

# Initialize git-branchless
echo
echo "Initializing git-branchless..."
git-branchless init

echo -e "${BLUE}Creating a commit graph for testing...${NC}"

# Main branch
echo "Main feature" > main.txt
git add main.txt
git commit -S -m "Add main feature"

# First feature branch with two commits
git checkout -b feature1
echo "Feature 1 commit" > feature1.txt
git add feature1.txt
git commit -S -m "Add feature1"

# Second feature branch based on main
git checkout main
git checkout -b feature2
echo "Feature 2 commit" > feature2.txt
git add feature2.txt
git commit -S -m "Add feature2"

# Create a third branch from main
git checkout main
git checkout -b feature3
echo "Feature 3 commit" > feature3.txt
git add feature3.txt
git commit -S -m "Add feature3"

# Show the commit graph
echo -e "${BLUE}Commit graph before operations:${NC}"
git branchless smartlog

# SCENARIO 1: Moving a commit to change the branch structure
echo -e "${GREEN}Scenario 1: Moving a commit to change branch structure${NC}"
echo "Moving feature2 to be based on feature1..."

git branchless move -d feature2 -s feature1

# Show the updated commit graph
echo -e "${BLUE}Commit graph after moving feature2:${NC}"
git branchless smartlog

# Verify signatures of the moved commit
echo -e "${GREEN}Verifying signatures in moved branch:${NC}"
git checkout feature2
echo "Checking signature of feature2 tip:"
git verify-commit HEAD

# SCENARIO 2: Test restacking after base modification
echo -e "${GREEN}Scenario 2: Testing restack with divergent branches${NC}"
echo "Modifying feature1 to cause feature2 to need restacking..."

# Modify feature1
git checkout feature1
echo "Modified feature1" >> feature1.txt
git add feature1.txt
git commit --amend -S -m "Modified feature1 (amended)"

# Show the broken commit graph
echo -e "${BLUE}Commit graph with abandoned branch:${NC}"
git branchless smartlog

# Restack the commits
echo "Restacking all abandoned commits..."
git branchless restack

# Show the fixed commit graph
echo -e "${BLUE}Commit graph after restacking:${NC}"
git branchless smartlog

# Verify signatures of restacked commits
echo -e "${GREEN}Verifying signatures after restack:${NC}"
git checkout feature2
echo "Checking signature of feature2 tip after restack:"
git verify-commit HEAD

# SCENARIO 3: Test interactive record with signing
echo -e "${GREEN}Scenario 3: Testing record with signing${NC}"

git checkout feature3
echo "Additional content" >> feature3.txt
git branchless record -m "Update feature3 with record command"

# Check signature of the new commit
echo "Checking signature of commit created with record command:"
git verify-commit HEAD

# SCENARIO 4: Testing complex move operation
echo -e "${GREEN}Scenario 4: Testing a more complex move operation${NC}"
echo "Moving feature3 to be based on feature2..."

git branchless move -d feature3 -s feature2

# Show the updated commit graph
echo -e "${BLUE}Final commit graph:${NC}"
git branchless smartlog

# Verify signature
echo "Checking signature of feature3 tip after move:"
git verify-commit HEAD

echo
echo -e "${GREEN}Advanced GPG testing completed!${NC}"
echo "The test repository is at: $TEST_DIR"
echo "You can explore the commit graph with: cd $TEST_DIR && git branchless smartlog"
echo "You can delete it when you're done with: rm -rf $TEST_DIR" 
