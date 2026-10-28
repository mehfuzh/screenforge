@echo off
rem Inno Setup SignTool entry point: runs scripts/sign-windows.sh under the Git Bash that built the bundle.
bash "%~dp0sign-windows.sh" %*
