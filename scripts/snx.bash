# Bash completion loader for a source checkout.
# Installed packages contain a fully generated completion file.
if command -v snx >/dev/null 2>&1; then
  eval "$(snx completion bash)"
fi
