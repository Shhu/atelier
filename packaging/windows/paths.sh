# Sourced by the build scripts: where the Windows build goes, from WSL.
# WIN_HOME: the Windows user's profile folder (C:\Users\…), and its WSL path.
WIN_HOME=$(cd /mnt/c && cmd.exe /c 'echo %USERPROFILE%' | tr -d '\r')
WIN_HOME_WSL=$(wslpath "$WIN_HOME")
WIN_TARGET="$WIN_HOME\\.atelier-target"
WIN_TARGET_WSL="$WIN_HOME_WSL/.atelier-target"
