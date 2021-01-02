import setuptools
from setuptools_rust import RustExtension


if __name__ == "__main__":
    setuptools.setup(
        rust_extensions=[
            RustExtension(
                "branchless.branchlessrust", "Cargo.toml", quiet=False, debug=True
            )
        ]
    )
