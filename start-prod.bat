@echo off
chcp 65001 > nul
echo ========================================================
echo   Сборка и запуск Balabol в Production режиме
echo ========================================================

echo 1. Сборка клиента и сервера...
cd client && call npm run build && cd ../server && call npm run build && cd ..

echo.
echo 2. Запуск единого сервера на порту 3001...
cd server && node dist/index.js
