@echo off
chcp 65001 > nul
echo ========================================================
echo   Запуск Balabol 2.0 (Rust P2P Core + Tauri/Svelte)
echo ========================================================
echo 1. Запуск Stateless Rendezvous Сервера (порт 4001)...
start "Balabol Rendezvous Tracker" cmd /k "cd rendezvous && cargo run"

timeout /t 2 /nobreak > nul

echo 2. Запуск Svelte Desktop UI...
start "Balabol UI" cmd /k "cd desktop && npm run dev"

echo.
echo Rendezvous Tracker: http://localhost:4001/health
echo Desktop Web UI:     http://localhost:5173
echo.
echo Для запуска в окне приложения Tauri (с доступом к аудио и P2P):
echo   cd desktop && npm.cmd run tauri dev
echo ========================================================
