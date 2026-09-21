@echo off
chcp 65001 > nul
echo ========================================================
echo   Запуск Balabol в режиме разработки (Dev Mode)
echo ========================================================
echo 1. Запуск Backend сервера (порт 3001)...
start "Balabol Server" cmd /k "cd server && npm run dev"

timeout /t 2 /nobreak > nul

echo 2. Запуск Frontend интерфейса (порт 5173)...
start "Balabol Client" cmd /k "cd client && npm run dev"

echo.
echo Приложение запустится в браузере по адресу: http://localhost:5173
echo ========================================================
