@echo off
chcp 65001 > nul
title Penplot-Gcoder Setup

echo ============================================================
echo  Penplot-Gcoder セットアップ ^& 起動
echo ============================================================
echo.

:: Python が使えるか確認
python --version > nul 2>&1
if errorlevel 1 (
    echo [ERROR] Python が見つかりません。
    echo Python 3.10 以上をインストールしてください:
    echo   https://www.python.org/downloads/
    echo.
    pause
    exit /b 1
)

echo [1/2] 依存パッケージをインストール中...
python -m pip install --upgrade pip --quiet
python -m pip install -r requirements.txt
if errorlevel 1 (
    echo.
    echo [ERROR] パッケージのインストールに失敗しました。
    echo ネットワーク接続を確認して再試行してください。
    echo.
    pause
    exit /b 1
)

echo.
echo [2/2] Penplot-Gcoder を起動します...
echo.
python main.py

if errorlevel 1 (
    echo.
    echo [ERROR] 起動中にエラーが発生しました。
    pause
)
