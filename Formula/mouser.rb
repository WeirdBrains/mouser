class Mouser < Formula
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  version "1.5.0"
  on_macos do
    on_arm do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/mouser-aarch64-apple-darwin.tar.gz"
      sha256 "d29188eb18db75fccbda14c064e47c06f92758256f73622f2ceb0ac97fe37aaf"
    end
    on_intel do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.5.0/mouser-x86_64-apple-darwin.tar.gz"
      sha256 "720f712402d791ddbcb1a30ca3a0c8533f7be5c3ce3bd875a3ca38c516221cfa"
    end
  end
  def install
    bin.install "mouser-app" => "mouser"
    (lib/"mouser").install "engines"
  end
end
