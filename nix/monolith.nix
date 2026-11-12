{monolith}:
monolith.overrideAttrs (old: {
  # Monolith 2.12 includes the local SVG fix; the old source patch no longer
  # matches its API. Keep the regression check against the installed binary.
  postInstall =
    (old.postInstall or "")
    + ''
      printf '%s\n' '<svg><defs><path id="local" /></defs><use href="#local" /></svg>' \
        > "$TMPDIR/local-svg.html"
      "$out/bin/monolith" "$TMPDIR/local-svg.html" -o "$TMPDIR/local-svg-output.html"
      grep -F 'href="#local"' "$TMPDIR/local-svg-output.html"
      if grep -F 'data:text/html' "$TMPDIR/local-svg-output.html"; then
        echo 'Local SVG reference embedded the HTML document' >&2
        exit 1
      fi
    '';
})
