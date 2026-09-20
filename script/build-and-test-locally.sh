#!/bin/bash

cargo build --profile release-fast -p zed --bin zed &&
  cp target/release-fast/zed /Applications/Zednotes.app/Contents/MacOS/zed &&
  codesign --force --deep --sign - /Applications/Zednotes.app &&
  open /Applications/Zednotes.app
