{monolith}:
monolith.overrideAttrs (old: {
  patches = (old.patches or []) ++ [./monolith-local-svg.patch];
})
