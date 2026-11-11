class Mouser < Formula
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  version "1.7.0"
  on_macos do
    on_arm do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.7.0/mouser-aarch64-apple-darwin.tar.gz"
      sha256 "903c98042f69640ed88e05c0c8992dee6ec15bdd659252b1154af4a65331734f"
    end
    on_intel do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.7.0/mouser-x86_64-apple-darwin.tar.gz"
      sha256 "737a6c81ec95e635197b57f316bf9de717864ba7fb0f12126d1ed450a960b6d7"
    end
  end
  def install
    bin.install "mouser-app" => "mouser"
    (lib/"mouser").install "engines"
  end
end
