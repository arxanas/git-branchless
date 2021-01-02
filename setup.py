import setuptools
from setuptools_rust import RustExtension


if __name__ == "__main__":
    setuptools.setup(
        rust_extensions=[
            RustExtension("branchless.rust", "Cargo.toml", quiet=False, debug=True)
        ]
    )
