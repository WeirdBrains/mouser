cask "mouser" do
  version "1.5.0"
  on_arm do
    sha256 "1c57dbf7259681c2e7484909f0a620551b3881b3c9fa1dbfc287fe00bd7e6b4f"
    url "https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/Mouser-aarch64-apple-darwin.dmg"
  end
  on_intel do
    sha256 "9f7488a28e7feaa8fb1f9f045b5b699a7162cf55fe0d48abe51bc027d5cf95de"
    url "https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/Mouser-x86_64-apple-darwin.dmg"
  end
  name "Mouser"
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  auto_updates false
  app "Mouser.app"
end
