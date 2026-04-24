# Windows Nanoserver base image template
#
# This is a *template* — customize it for your specific workload. Nanoserver is
# the smallest Windows base (~100MB) but lacks package managers (no
# chocolatey/winget) and ships no PowerShell. It is intended for self-contained,
# statically-compiled Windows binaries (e.g. Go, .NET AOT, Rust MSVC toolchain).
#
# If you need chocolatey, winget, PowerShell, or the full .NET SDK at runtime,
# use the servercore template instead.
#
# The COPY / CMD lines below are placeholders — replace them with the path to
# your actual binary.

FROM mcr.microsoft.com/windows/nanoserver:ltsc2022

# ContainerAdministrator is the built-in admin principal on nanoserver
USER ContainerAdministrator

WORKDIR C:\\app

# Placeholder — replace with your actual binary
COPY ./app.exe C:\\app\\app.exe

# Default command — override with your entry point
CMD ["C:\\app\\app.exe"]
