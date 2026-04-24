# Windows Server Core base image template
#
# This is a *template* — customize it for your specific workload. Server Core
# is the larger (~1.5GB) Windows base that bundles PowerShell, Windows
# package-management primitives, and is compatible with chocolatey and winget
# (unlike nanoserver, which ships no package managers and no PowerShell).
#
# Use this image when you need:
#   - chocolatey / winget at build time
#   - PowerShell scripts at runtime
#   - The full .NET SDK (not just the runtime)
#   - Legacy Windows components (WMI tooling, IIS, etc.)
#
# The SHELL instruction switches the default to PowerShell so subsequent RUN
# instructions accept PowerShell syntax. The COPY / CMD lines below are
# placeholders — replace them with the path to your actual binary.

FROM mcr.microsoft.com/windows/servercore:ltsc2022

# Servercore bundles PowerShell — make it the default shell for RUN commands.
SHELL ["powershell", "-Command"]

# ContainerAdministrator is the built-in admin principal on servercore
USER ContainerAdministrator

WORKDIR C:\\app

# Placeholder — replace with your actual binary
COPY ./app.exe C:\\app\\app.exe

# Default command — override with your entry point
CMD ["C:\\app\\app.exe"]
