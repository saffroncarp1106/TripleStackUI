# Triple Stack Skin Maker V0.3.0

This is the Windows x64 Rust edition of Triple Stack Skin Maker. This release is rewritten in Rust and requires no Python installation or Python dependencies. The main window uses OpenGL, including its controls, animation and image previews. Windows supplies the title bar and file dialogs.

## Run

Extract the Windows ZIP and open TripleStack.exe. The app uses the OpenGL driver installed on Windows.

Select an osu! skin folder, adjust Options if needed, and click Confirm. Copy & Process is on by default and creates a sibling folder ending in @3xStack. The save dialog exports the result as an .osk file.

## Build from source

Keep hitcircle.png, followpoint.png, and icon.ico in the parent directory of this Cargo project. Use the Windows x64 MSVC Rust toolchain with Visual Studio C++ Build Tools and a Windows SDK.

Double-click build.bat or run it from a terminal. It places the release executable in target\windows-x64-opengl. Open TripleStack.exe in that folder to run the build.



Build commands are run from the TripleStack directory.

