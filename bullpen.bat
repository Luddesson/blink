@echo off
REM Wrapper to call Bullpen CLI via WSL2 Ubuntu
REM Usage: bullpen.bat <args...>
wsl -d Ubuntu -- bullpen %*
