@echo off
chcp 65001 > nul

:: Проверка и автоматический запрос прав Администратора (UAC)
net session >nul 2>&1
if %errorLevel% neq 0 (
    echo Запрос прав Администратора...
    powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process cmd -ArgumentList '/c \"\"%~f0\"\"' -Verb RunAs"
    exit /b
)

echo ====================================================================
echo  Настройка Брандмауэра Windows для Balabol P2P
echo ====================================================================
echo.

echo 1. Удаление старых правил...
netsh advfirewall firewall delete rule name="balabol-desktop.exe" > nul 2>&1
netsh advfirewall firewall delete rule name="balabol.exe" > nul 2>&1
netsh advfirewall firewall delete rule name="balabol-udp-9987" > nul 2>&1

echo 2. Добавление разрешающих правил для dist-release\balabol.exe...
netsh advfirewall firewall add rule name="balabol.exe" dir=in action=allow program="%~dp0dist-release\balabol.exe" enable=yes profile=any
netsh advfirewall firewall add rule name="balabol.exe" dir=out action=allow program="%~dp0dist-release\balabol.exe" enable=yes profile=any

echo 3. Разрешение входящего UDP трафика для P2P соединения...
netsh advfirewall firewall add rule name="balabol-udp-9987" dir=in action=allow protocol=UDP localport=9987 enable=yes profile=any

echo.
echo ====================================================================
echo [УСПЕХ] Брандмауэр Windows успешно настроен для Balabol!
echo ====================================================================
echo Исполняемый файл dist-release\balabol.exe и UDP порты разблокированы.
echo.
pause
