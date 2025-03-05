#!/bin/bash
set -eo pipefail

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}Testing git-branchless GPG signing functionality${NC}"
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

# Create a branch to work with
git checkout -b feature

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

echo
echo "Creating a series of commits to test with..."

# Create some test files and commits
echo "First content" > file1.txt
git add file1.txt
git commit -S -m "Add file1.txt (signed)"

echo "Second content" > file2.txt
git add file2.txt
git commit -S -m "Add file2.txt (signed)"

echo "Third content" > file3.txt
git add file3.txt
git commit -S -m "Add file3.txt (signed)"

# Save the hash of the latest commit
LATEST_COMMIT=$(git rev-parse HEAD)

# Initialize git-branchless
echo
echo "Initializing git-branchless..."
git-branchless init

# Show current state
echo
echo "Current commit stack:"
git branchless smartlog

# Create a commit to be moved or restacked
git checkout HEAD~1
echo "Feature content" > feature.txt
git add feature.txt
git commit -S -m "Add feature.txt (signed, to be moved)"

# Get the hash of the new feature commit
FEATURE_COMMIT=$(git rev-parse HEAD)

# Show current state again
echo
echo "Commit stack with a commit to be moved:"
git branchless smartlog

# Test git move
echo
echo -e "${GREEN}Testing git move with signed commits...${NC}"
echo "Moving the 'feature.txt' commit to be on top of the latest commit..."
git branchless move -d "$FEATURE_COMMIT" -s "$LATEST_COMMIT"

# Show the result
echo
echo "Commit stack after move:"
git branchless smartlog

# Verify signatures
echo
echo -e "${GREEN}Verifying signatures...${NC}"
git checkout @ # Go to the tip of the stack
echo "Checking signature of the moved commit:"
git verify-commit HEAD

echo
echo "Checking signature of the previous commit:"
git verify-commit HEAD~1

# Test git restack
echo
echo -e "${GREEN}Testing git restack with signed commits...${NC}"
# Create a situation that would require restacking
git checkout HEAD~2
echo "Updated content" >> file1.txt
git add file1.txt
git commit --amend -S -m "Update file1.txt (amended, signed)"

echo
echo "Commit stack after amending a commit (should show abandoned commits):"
git branchless smartlog

echo
echo "Restacking commits..."
git branchless restack

echo
echo "Commit stack after restacking:"
git branchless smartlog

# Verify signatures again
echo
echo -e "${GREEN}Verifying signatures after restack...${NC}"
echo "Checking signatures of restacked commits:"
for commit in $(git log --format=%H -n 3); do
    echo "Commit: $(git log -1 --format=%s $commit)"
    git verify-commit $commit || echo "Signature verification failed!"
    echo
done

echo
echo -e "${GREEN}Test completed!${NC}"
echo "The test repository is at: $TEST_DIR"
echo "You can delete it when you're done with: rm -rf $TEST_DIR" 
