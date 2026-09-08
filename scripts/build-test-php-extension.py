#!/usr/bin/env python3
"""Build a Linux-only PHP test extension against a matching SDK."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--php-config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cc", default="cc")
    parser.add_argument("--source", type=Path,
                        default=Path(__file__).resolve().parent / "fixtures/php-native-fault.c")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("this native fault fixture has only been validated on Linux")
    config = args.php_config.resolve(strict=True)
    source = args.source.resolve(strict=True)
    flags = shlex.split(subprocess.check_output([str(config), "--includes"], text=True))
    output = args.output.resolve()
    command = [args.cc, "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
               "-Wno-unused-parameter", *flags, str(source), "-o", str(output)]
    subprocess.run(command, check=True)
    print(json.dumps({"command": command,
                      "php_version": subprocess.check_output([str(config), "--version"], text=True).strip(),
                      "compiler": subprocess.check_output([args.cc, "--version"], text=True).splitlines()[0],
                      "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                      "extension_sha256": hashlib.sha256(output.read_bytes()).hexdigest()}))


if __name__ == "__main__":
    main()
