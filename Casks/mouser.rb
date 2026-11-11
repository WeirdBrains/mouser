cask "mouser" do
  version "1.7.0"
  on_arm do
    sha256 "4ab5e73d1638f1bac424b8bfe6838ab2628103c0e0d3f8534974a0c83dcbd2bb"
    url "https://github.com/WeirdBrains/mouser/releases/download/v1.7.0/Mouser-aarch64-apple-darwin.dmg"
  end
  on_intel do
    sha256 "bf0676b326accd4fbc62397ec8dc5322ecdc41acd3fd90fc82cda307b505ca17"
    url "https://github.com/WeirdBrains/mouser/releases/download/v1.7.0/Mouser-x86_64-apple-darwin.dmg"
  end
  name "Mouser"
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  auto_updates false
  app "Mouser.app"
end
