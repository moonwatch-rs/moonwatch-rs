#!/usr/bin/env python3
"""
Build x86_64-unknown-linux-gnu using local Cargo.

This is for native compilation on Linux.

"""

import os.path as op
import os
import subprocess
import shutil

TARGET_TRIPLE = "x86_64-unknown-linux-gnu"
PKGID_URL = subprocess.check_output(["cargo", "pkgid"]).decode("utf-8").strip()
PACKAGE_NAME_AND_VERSION = PKGID_URL.split("/")[-1].replace("#", "_")
ARCHIVE_NAME = f"{PACKAGE_NAME_AND_VERSION}_Linux-x86-64"
OUTPUT_ARCHIVE_PATH = op.abspath(f"build/{ARCHIVE_NAME}.tar.gz")

output_build_dir = "./build"
root_dir = op.abspath(op.dirname(__file__))
output_dir = op.join(root_dir, f"build/{ARCHIVE_NAME}")
if op.exists(output_dir):
    shutil.rmtree(output_dir)
os.makedirs(output_dir)

subprocess.check_call(["cargo", "build", "--locked", "--release", "--target", f"{TARGET_TRIPLE}"])
build_dir = op.join(root_dir, f"target/{TARGET_TRIPLE}/release")
share_dir = op.join(root_dir, "share")

shutil.copy(op.join(build_dir, "moonwatcher"), output_dir)
shutil.copy(op.join(share_dir, "default-main-config-unix.json"), op.join(output_dir, "main_config.json"))
shutil.copy(op.join(share_dir, "default-recorder-config.json"), op.join(output_dir, "recorder_config.json"))
shutil.copy(op.join(share_dir, "default-pipeline-config.json"), op.join(output_dir, "pipeline_config.json"))
shutil.copy(op.join(share_dir, "install_unix.py"), output_dir)
shutil.copy(op.join(share_dir, "moonwatch-rs.service"), output_dir)

if op.exists(OUTPUT_ARCHIVE_PATH):
    os.remove(OUTPUT_ARCHIVE_PATH)
subprocess.check_call(["tar", "cvfz", OUTPUT_ARCHIVE_PATH, ARCHIVE_NAME], cwd=output_build_dir)
print("Created archive with build:", OUTPUT_ARCHIVE_PATH)
