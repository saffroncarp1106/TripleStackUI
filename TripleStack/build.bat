@echo off
setlocal EnableExtensions DisableDelayedExpansion
pushd "%~dp0" || exit /b 1

set "target=x86_64-pc-windows-msvc"
set "bundle=target\windows-x64-opengl"

where.exe cargo.exe >nul 2>&1
if errorlevel 1 (
    echo Cargo was not found. Install Rust and reopen this terminal. 1>&2
    goto failed
)

cargo build --locked --release --target "%target%" --target-dir target
if errorlevel 1 goto failed

if not exist "%bundle%\" mkdir "%bundle%"
if errorlevel 1 goto failed
copy /Y "target\%target%\release\triplestack.exe" "%bundle%\TripleStack.exe" >nul
if errorlevel 1 goto failed

echo Built "%CD%\%bundle%\TripleStack.exe"
popd
exit /b 0

:failed
echo Triple Stack build failed. 1>&2
popd
exit /b 1
