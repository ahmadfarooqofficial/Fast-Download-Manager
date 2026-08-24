@echo off
REM ===========================================================================
REM  FDM - Fast Download Manager :: build and install from source
REM
REM  This file is for people who cloned the repository.
REM
REM  If you just want to USE FDM, do not run this. Download the ready-made
REM  installer instead -- one file, double-click, done:
REM      https://github.com/ahmadfarooq/fdm/releases
REM
REM  What this does, with no further input from you:
REM      1. Elevates itself (needed to install compilers and register the
REM         browser bridge).
REM      2. Installs anything missing: Rust, Node.js, Visual Studio Build
REM         Tools, Inno Setup.
REM      3. Builds FDM in release mode.
REM      4. Compiles FDM-Setup-<version>.exe.
REM      5. Runs that installer.
REM
REM  First run downloads a few GB of compilers and takes 15-40 minutes.
REM  Later runs take about a minute.
REM ===========================================================================

setlocal EnableExtensions
cd /d "%~dp0"

echo.
echo   FDM - Fast Download Manager
echo   Build and install from source
echo   ------------------------------------------------------------
echo.

REM --- Elevate ---------------------------------------------------------------
REM `net session` fails for non-administrators; it is the most portable check
REM that does not depend on the locale of whoami output.
net session >nul 2>&1
if not errorlevel 1 goto :elevated

echo   Administrator rights are required to install compilers and to register
echo   the browser bridge under HKLM. Asking Windows for permission...
echo.
powershell -NoProfile -Command ^
  "Start-Process -FilePath '%ComSpec%' -ArgumentList '/k','\"%~f0\"' -Verb RunAs"
if errorlevel 1 (
  echo.
  echo   Could not elevate. Right-click install.bat and pick
  echo   "Run as administrator" instead.
  echo.
  pause
)
exit /b

:elevated
cd /d "%~dp0"

REM --- Sanity: are we actually in a checkout? --------------------------------
if not exist "scripts\build-installer.ps1" (
  echo   ERROR: scripts\build-installer.ps1 is missing.
  echo   Run install.bat from the root of a full FDM checkout.
  echo.
  pause
  exit /b 1
)

REM --- winget is the one thing we cannot install for you ---------------------
where winget >nul 2>&1
if errorlevel 1 (
  echo   ERROR: winget ^(App Installer^) was not found.
  echo.
  echo   Every other dependency can be installed automatically, but winget is
  echo   how that happens. Install "App Installer" from the Microsoft Store,
  echo   then run this file again:
  echo.
  echo     https://apps.microsoft.com/detail/9nblggh4nns1
  echo.
  pause
  exit /b 1
)

REM --- Node.js, needed to rasterise the icons -------------------------------
where node >nul 2>&1
if errorlevel 1 (
  echo   Installing Node.js...
  winget install --id OpenJS.NodeJS.LTS --silent --accept-source-agreements ^
    --accept-package-agreements --disable-interactivity
  REM winget cannot update the PATH of an already-running shell, so re-read it
  REM from the registry rather than telling the user to reopen the window.
  for /f "tokens=2,*" %%A in ('reg query "HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment" /v Path 2^>nul ^| findstr /i "Path"') do set "MACHPATH=%%B"
  if defined MACHPATH set "PATH=%MACHPATH%;%PATH%"
)

REM --- Hand off. build-installer.ps1 owns the rest, including installing -----
REM --- Rust, Build Tools and Inno Setup if they are missing. -----------------
echo   Building. Leave this window open.
echo.
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\build-installer.ps1" -AllowPartial
if errorlevel 1 (
  echo.
  echo   Build failed. The reason is printed above.
  echo.
  echo   If it mentions a missing linker, Visual Studio Build Tools is still
  echo   installing in the background -- wait for it to finish, then run this
  echo   file again.
  echo.
  pause
  exit /b 1
)

REM --- Find and run the setup file we just built ----------------------------
set "SETUPEXE="
for /f "delims=" %%F in ('dir /b /o-d "installer\output\FDM-Setup-*.exe" 2^>nul') do (
  if not defined SETUPEXE set "SETUPEXE=installer\output\%%F"
)

if not defined SETUPEXE (
  echo   Build reported success but no setup file was found in installer\output.
  echo.
  pause
  exit /b 1
)

echo.
echo   ------------------------------------------------------------
echo   Built: %SETUPEXE%
echo   ------------------------------------------------------------
echo.
echo   Starting the installer now. Click through it and you are done.
echo.
start "" "%SETUPEXE%"

echo   One last thing, and it is Chrome's rule rather than ours: when your
echo   browser asks whether to enable the FDM extension, click Enable.
echo   Chrome does not let any installer do that step for you.
echo.
pause
endlocal
