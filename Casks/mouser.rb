cask "mouser" do
  version "0.7.2"
  on_arm do
    sha256 ""
    url "https://github.com/WeirdBrains/mouser/releases/download/v0.7.2/Mouser-aarch64-apple-darwin.dmg"
  end
  on_intel do
    sha256 ""
    url "https://github.com/WeirdBrains/mouser/releases/download/v0.7.2/Mouser-x86_64-apple-darwin.dmg"
  end
  name "Mouser"
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  auto_updates false
  app "Mouser.app"
end
