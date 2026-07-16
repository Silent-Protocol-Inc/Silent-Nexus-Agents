# Fish completion loader for a source checkout.
# Installed packages contain a fully generated completion file.
if type -q snx
    snx completion fish | source
end
