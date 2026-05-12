class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.0/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "55941fffb6296ea202a2a79cf931def196eaa610292519c5ace62c94402c6e8c"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.0/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "8f32ed29f26297a5aadc3f9d8cdda660d56545ea8ba54ebd4acebe72d478f64d"
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
