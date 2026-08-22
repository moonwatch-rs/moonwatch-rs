@echo Moonwatch.rs installer
@echo ----------------------

@set MOONWATCHDIR=%USERPROFILE%\.moonwatch-rs

@echo Installing into %MOONWATCHDIR%

mkdir "%MOONWATCHDIR%"

copy moonwatcher.exe "%MOONWATCHDIR%"

@for %%C in (main_config.json recorder_config.json pipeline_config.json) do @(
    if exist "%MOONWATCHDIR%\%%C" (
        echo %%C already exists, not copying default
    ) else (
        echo copying default %%C
        copy %%C "%MOONWATCHDIR%"
    )
)

@echo Installing to Startup menu

@set SHORTCUT='%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\moonwatcher.lnk'
@set WORKINGDIRECTORY='%MOONWATCHDIR%'
@set TARGET='%MOONWATCHDIR%\moonwatcher.exe'
@set ARGUMENTS='watch'
@set DESCRIPTION='Moonwatch.rs daemon'
@set PWS=powershell.exe -ExecutionPolicy Bypass -NoLogo -NonInteractive -NoProfile

%PWS% -Command "$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut(%SHORTCUT%); $S.WorkingDirectory = %WORKINGDIRECTORY%; $S.TargetPath = %TARGET%; $S.Arguments = %ARGUMENTS%; $S.Description = %DESCRIPTION%; $S.Save()"

@echo Installation is finished.
@pause
