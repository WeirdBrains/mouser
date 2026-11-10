class Mouser < Formula
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  version "1.6.0"
  on_macos do
    on_arm do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.6.0/mouser-aarch64-apple-darwin.tar.gz"
      sha256 "81007fdb3476c90e89a01f4d190f832eb679e6186c32bd985b061e16d27c72ed"
    end
    on_intel do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.6.0/mouser-x86_64-apple-darwin.tar.gz"
      sha256 "41a03f5ca9a1410cf5f836e0c235898ff30762e5a1c3fa9bdde203391b3481b9"
    end
  end
  def install
    bin.install "mouser-app" => "mouser"
    (lib/"mouser").install "engines"
  end
end
