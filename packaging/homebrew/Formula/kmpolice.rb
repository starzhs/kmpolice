class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.2/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "d4629f33d67db455e9d75c04c67c1e5fe926e32b65443696fa0716622e62cedd"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.2/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "9484391682f625288d69034e8ba30d1fcc580265542a16c95fbfd7654935517a"
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
