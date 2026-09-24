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

echo 1. Удаление блокирующих правил balabol...
netsh advfirewall firewall delete rule name="balabol-desktop.exe" > nul 2>&1
netsh advfirewall firewall delete rule name="balabol.exe" > nul 2>&1
netsh advfirewall firewall delete rule name="balabol-udp-9987" > nul 2>&1

echo 2. Добавление разрешающих правил для balabol-desktop.exe...
netsh advfirewall firewall add rule name="balabol-desktop.exe" dir=in action=allow program="%~dp0dist-release\balabol-desktop.exe" enable=yes profile=any
netsh advfirewall firewall add rule name="balabol-desktop.exe" dir=out action=allow program="%~dp0dist-release\balabol-desktop.exe" enable=yes profile=any

echo 3. Добавление разрешающего правила для входящего UDP порта 9987...
netsh advfirewall firewall add rule name="balabol-udp-9987" dir=in action=allow protocol=UDP localport=9987 enable=yes profile=any

echo.
echo ====================================================================
echo [УСПЕХ] Брандмауэр Windows успешно разблокирован для Balabol!
echo ====================================================================
echo Блокирующие правила удалены, входящий порт 9987 разрешен.
echo Теперь ноутбук сможет связаться с ПК.
echo.
pause
