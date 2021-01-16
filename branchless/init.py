"""Install any hooks, aliases, etc. to set up Branchless in this repo."""
from .rust import py_init as init

# For flake8.
_ = init
