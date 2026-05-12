class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.4/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "aa57847e89d56e17a680827e7809f5a8c628ae9fc7d2d44a4b1bbf4741d35b65"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.4/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "15973fcedb2c78e2f5e795d56554786873e44f4698d8b58e297e752cdbb40c23"
    end
  end

  def install
    bin.install "kmpolice"
  end

  test do
    output = shell_output("#{bin}/kmpolice --help")
    assert_match "Usage: kmpolice", output
  end
end
