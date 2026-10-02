# Adapted from ms-todo packaging/homebrew/ms-todo.rb @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
# Changes: the two macOS archives only (reading browser cookies is
# macOS-only); no alias; no license line (the repository has none yet).
# scripts/render_homebrew_formula.sh fills in the placeholders, and the
# release workflow pushes the result to planetaryescape/homebrew-chatgpt-cli.
class Chatgpt < Formula
  desc "Unofficial CLI and TUI for searching and managing ChatGPT history"
  homepage "https://github.com/planetaryescape/chatgpt-cli"
  version "__VERSION__"

  depends_on :macos

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/planetaryescape/chatgpt-cli/releases/download/v#{version}/chatgpt-v#{version}-macos-aarch64.tar.gz"
      sha256 "__SHA256_MACOS_AARCH64__"
    else
      url "https://github.com/planetaryescape/chatgpt-cli/releases/download/v#{version}/chatgpt-v#{version}-macos-x86_64.tar.gz"
      sha256 "__SHA256_MACOS_X86_64__"
    end
  end

  def install
    bin.install "chatgpt"
  end

  test do
    assert_match "chatgpt #{version}", shell_output("#{bin}/chatgpt --version")
  end
end
