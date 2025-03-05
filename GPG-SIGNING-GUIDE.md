# GPG Signing Functionality Guide

This guide explains how to test that the GPG signing functionality in this git-branchless fork works correctly.

## Automated Testing

### Basic Test Script

You can run the provided basic test script to automatically test GPG signing functionality:

```bash
./test-gpg-signing.sh
```

This script will:

1. Create a test repository
2. Configure GPG signing
3. Create signed commits
4. Test `git move` and `git restack` operations
5. Verify signatures are preserved

### Advanced Subtree Test Script

For testing more complex scenarios with commit subtrees and divergent development:

```bash
./test-advanced-gpg-subtrees.sh
```

This script tests:

1. Moving entire subtrees of commits while preserving signatures
2. Restacking multiple divergent branches simultaneously
3. Using advanced revset operations to manipulate specific commit groups
4. Verifying signatures are maintained across all operations

## Manual Testing

If you prefer to test the functionality manually, follow these steps:

### 1. Setup Test Repository

```bash
# Create a test repository
mkdir test-gpg-branchless
cd test-gpg-branchless
git init

# Configure git for the test
git config user.name "Your Name"
git config user.email "your.email@example.com"
git config user.signingkey YOUR_GPG_KEY_ID
git config commit.gpgsign true

# Create initial commit
echo "# Test Repository" > README.md
git add README.md
git commit -S -m "Initial commit"

# Initialize git-branchless
git-branchless init
```

### 2. Test Case 1: Moving Signed Commits

```bash
# Create a series of signed commits
echo "Feature A" > featureA.txt
git add featureA.txt
git commit -S -m "Add Feature A"

echo "Feature B" > featureB.txt
git add featureB.txt
git commit -S -m "Add Feature B"

# Create a branch to move later
git checkout HEAD~1
echo "Feature C" > featureC.txt
git add featureC.txt
git commit -S -m "Add Feature C"

# View the commit graph
git branchless smartlog

# Move the Feature C commit on top of Feature B
git branchless move -d "Add Feature C" -s "@"

# Check the commit graph
git branchless smartlog

# Verify signatures of the moved commits
git checkout @
git verify-commit HEAD  # Should show a valid signature for Feature C
git verify-commit HEAD~1  # Should show a valid signature for Feature B
```

### 3. Test Case 2: Restacking Signed Commits

```bash
# Create a stack of signed commits
git checkout master
echo "Base feature" > base.txt
git add base.txt
git commit -S -m "Add base feature"

echo "Dependent feature 1" > dep1.txt
git add dep1.txt
git commit -S -m "Add dependent feature 1"

echo "Dependent feature 2" > dep2.txt
git add dep2.txt
git commit -S -m "Add dependent feature 2"

# Amend a commit in the middle of the stack
git checkout HEAD~1
echo "Modified content" >> dep1.txt
git add dep1.txt
git commit --amend -S -m "Modified dependent feature 1"

# View the broken commit graph
git branchless smartlog
# Should show that "Add dependent feature 2" is abandoned

# Restack the commits
git branchless restack

# View the fixed commit graph
git branchless smartlog

# Verify signatures
git checkout @
git log -n 3 --format="%H %s"
# For each commit hash, verify the signature:
git verify-commit <COMMIT_HASH>
```

### 4. Test Case 3: Interactive Record with Signing

```bash
# Create some changes
echo "New content" > new_file.txt
echo "More content" >> README.md

# Record the changes with signing
git branchless record -m "Record with signing"

# Verify the commit has a signature
git verify-commit HEAD
```

### 5. Test Case 4: Advanced Subtree Operations

```bash
# Create a complex branching structure with multiple feature branches
git checkout master
git checkout -b feature1
echo "Feature 1" > feature1.txt
git add feature1.txt
git commit -S -m "Add Feature 1"

# Add a child commit to feature1
echo "Feature 1 update" >> feature1.txt
git add feature1.txt
git commit -S -m "Update Feature 1"

# Create another branch from master
git checkout master
git checkout -b feature2
echo "Feature 2" > feature2.txt
git add feature2.txt
git commit -S -m "Add Feature 2"

# Create a child branch from feature2
git checkout -b feature2-child
echo "Feature 2 child" > feature2-child.txt
git add feature2-child.txt
git commit -S -m "Add Feature 2 child"

# View the structure
git branchless smartlog

# Store commit hashes for reference
FEATURE1_HEAD=$(git rev-parse feature1)
FEATURE2_HEAD=$(git rev-parse feature2)
FEATURE2_CHILD_HEAD=$(git rev-parse feature2-child)

# Move the entire feature2 subtree (including its child) onto feature1
git branchless move -d "ancestors($FEATURE2_HEAD) & descendants($FEATURE2_HEAD, $FEATURE2_CHILD_HEAD)" -s "$FEATURE1_HEAD"

# View the new structure
git branchless smartlog

# Verify the signatures on all moved commits
git checkout feature2
git verify-commit HEAD
git checkout feature2-child
git verify-commit HEAD
```

## Expected Results

For all operations, the expected result is that commits maintain or receive proper GPG signatures. You should see:

1. When using `git move`, the moved commit should maintain its signature
2. When using `git restack`, restacked commits should maintain their signatures
3. When using `git record`, new commits should be signed if signing is enabled
4. When moving subtrees of commits, all commits in the subtree should maintain their signatures
5. When restacking divergent branches, all commits should maintain their signatures

If any of these operations result in unsigned commits when they should be signed, then there's an issue with the GPG signing functionality in the fork.

## Troubleshooting

If you encounter issues with GPG signing:

1. Verify your GPG key is properly configured:

   ```bash
   git config --get user.signingkey
   ```

2. Test basic Git signing:

   ```bash
   echo "test" > test.txt
   git add test.txt
   git commit -S -m "Test signing"
   git verify-commit HEAD
   ```

3. Check if the global signing option is enabled:

   ```bash
   git config --get commit.gpgsign
   ```

4. If you're using a different signing program (like SSH keys), ensure it's properly configured:

   ```bash
   git config --get gpg.format
   ```

5. For debugging, you can enable more verbose GPG output:
   ```bash
   export GPG_TTY=$(tty)
   ```

## Advanced Revset Usage

git-branchless provides powerful revset expressions for selecting specific groups of commits:

```bash
# Move all commits that are descendants of commit A but not descendants of commit B
git branchless move -d "descendants(A) - descendants(B)" -s "target"

# Move all commits that are between A and B (inclusive)
git branchless move -d "A..B" -s "target"

# Move all commits that are ancestors of A but also descendants of B
git branchless move -d "ancestors(A) & descendants(B)" -s "target"

# Move a commit and all its children
git branchless move -d "A | descendants(A)" -s "target"
```

You can use these expressions to precisely control which commits are affected by your operations, while the GPG signing support in this fork ensures that signatures are preserved.
