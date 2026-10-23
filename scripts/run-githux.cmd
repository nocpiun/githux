@echo off
rem Cargo Windows target runner bridge (configured in .cargo/config.toml).
rem
rem This file is the first runner element, so Cargo resolves it relative to
rem .cargo/config.toml rather than the current directory. Cargo appends the executable
rem and its arguments; %* forwards them to PowerShell, and the environment's own
rem command line is read back there. %* is deliberately not parsed here: reading
rem [Environment]::GetCommandLineArgs() in PowerShell keeps quotes, percents, carets,
rem ampersands and Unicode intact.
rem
rem -NoProfile / -NonInteractive / -ExecutionPolicy Bypass apply to this process only;
rem nothing is changed for the system or the user.
powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0run-githux.ps1" --githux-runner-- %*
exit /b %ERRORLEVEL%
