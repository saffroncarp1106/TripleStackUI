@echo off
setlocal EnableExtensions DisableDelayedExpansion
pushd "%~dp0" || exit /b 1

set "target=x86_64-pc-windows-msvc"
set "bundle=target\windows-x64-opengl"

powershell.exe -NoProfile -Command "$exe='target\windows-x64-opengl\TripleStack.exe'; if (!(Test-Path -LiteralPath $exe)) { exit 1 }; $stamp=(Get-Item -LiteralPath $exe).LastWriteTimeUtc; $paths=@('src','assets/number-fonts.zip','assets/audio-examples.zip','assets/audio-examples.json','Cargo.toml','Cargo.lock','build.rs','triplestack.rc','build.bat','..\hitcircle.png','..\icon.ico'); foreach ($path in $paths) { if (!(Test-Path -LiteralPath $path)) { exit 1 }; $item=Get-Item -LiteralPath $path; if ($item.PSIsContainer) { $files=Get-ChildItem -LiteralPath $path -Recurse -File } else { $files=@($item) }; if (@($files | Where-Object { $_.LastWriteTimeUtc -gt $stamp }).Count -gt 0) { exit 1 } }; exit 0"
if not errorlevel 1 goto launch

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
:launch
start "" "%CD%\%bundle%\TripleStack.exe"
if errorlevel 1 goto failed
popd
exit /b 0

:failed
echo Triple Stack build failed. 1>&2
popd
exit /b 1
