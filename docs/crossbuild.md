Да, это возможно. Процесс сборки кода для одной платформы (Linux) под другую (macOS) называется кросс-компиляцией (cross-compilation).
Однако в случае с macOS есть важный нюанс: Apple требует использования её собственного компоновщика (linker) и пакета системных библиотек (SDK), распространяемых через Xcode, что усложняет прямую сборку на Linux.
Чтобы настроить такую компиляцию, используют один из двух основных путей:

Способ 1. Использование специального тулчейна (рекомендуемый)

Самый популярный способ решить проблему с библиотеками macOS на Linux — использовать проект osxcross. Он упаковывает SDK от Apple в понятный для Linux формат.
1. Скачайте macOS SDK (для этого потребуется учётная запись Apple Developer, чтобы загрузить Xcode, либо готовый SDK из проверенных репозиториев вроде tpoechtrager/osxcross).
2. Соберите osxcross на своей Linux-машине.
3. Добавьте целевую платформу в Rust via rustup:bash
rustup target add x86_64-apple-darwin # Для старых Mac на Intel
# или
rustup target add aarch64-apple-darwin # Для новых Mac на процессорах Apple Silicon (M1/M2/M3)
Используйте код с осторожностью.
4. Укажите путь к новому компоновщику в файле .cargo/config.toml:toml
[target.x86_64-apple-darwin]
linker = "x86_64-apple-darwin<version>-clang"

[target.aarch64-apple-darwin]
linker = "aarch64-apple-darwin<version>-clang"
Используйте код с осторожностью.
5. Скомпилируйте проект:bash
cargo build --target aarch64-apple-darwin --release
Используйте код с осторожностью.

Способ 2. Автоматизация через Docker (самый простой)

Если вы не хотите вручную настраивать osxcross, можно использовать готовые Docker-контейнеры. Самый известный инструмент — cross.
1. Установите cross:bash
cargo install cross --git https://github.com
Используйте код с осторожностью.
2. Запустите сборку (инструмент сам скачает нужный образ с настроенным macOS SDK и скомпилирует бинарник):bash
cross build --target aarch64-apple-darwin --release
Используйте код с осторожностью.

⚠️ Главное ограничение: Подпись приложения (Code Signing)

Даже если вы успешно скомпилируете .dylib файл или исполняемый бинарник под macOS на Linux, вы не сможете полноценно подписать его официальным сертификатом разработчика Apple.
Пользователи, которые скачают ваше приложение на Mac, увидят предупреждение системы безопасности (Gatekeeper), что программа повреждена или получена от неустановленного разработчика. Для обхода этого им придётся открывать приложение через контекстное меню (правой кнопкой мыши -> «Открыть»). Полную нотаризацию (notarization) и подпись приложения можно сделать только на реальном компьютере Mac или в облачном macOS-CI (например, GitHub Actions).
Под какую именно архитектуру Mac вы планируете собирать приложение (M-чипы или Intel)? Напишите, и я могу помочь составить пошаговую инструкцию по настройке конфигурационных файлов.

---

На NixOS настроить кросс-компиляцию под macOS значительно проще, чем на других дистрибутивах Linux. Благодаря декларативному подходу Nix, вам не нужно вручную собирать osxcross или скачивать SDK. Все необходимые тулчейны и заголовки уже упакованы в nixpkgs.
Поскольку целевая архитектура может быть любой, лучшим решением будет использовать временное окружение разработки (Nix Shell) со встроенными компиляторами для обеих архитектур Mac (x86_64-apple-darwin для Intel и aarch64-apple-darwin для Apple Silicon).

Шаг 1. Создание окружения shell.nix

Создайте в корне вашего Rust-проекта файл с именем shell.nix следующего содержания:
nix
{ pkgs ? import <nixpkgs> {} }:

let
  # Тулчейн для Intel Mac
  intelPkgs = import pkgs.path {
    crossSystem = {
      config = "x86_64-apple-darwin";
    };
  };

  # Тулчейн для Apple Silicon Mac (M1/M2/M3/M4)
  mSiliconPkgs = import pkgs.path {
    crossSystem = {
      config = "aarch64-apple-darwin";
    };
  };

in pkgs.mkShell {
  name = "rust-cross-macos";

  buildInputs = [
    pkgs.rustup
    pkgs.libiconv
  ];

  shellHook = ''
    # Экспортируем переменные для Cargo, чтобы он знал, какие линкеры использовать
    export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER="${intelPkgs.stdenv.cc}/bin/x86_64-apple-darwin-clang"
    export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER="${mSiliconPkgs.stdenv.cc}/bin/aarch64-apple-darwin-clang"

    echo " Окружение для кросс-компиляции на macOS готово!"
    echo "Используйте 'cargo build --target x86_64-apple-darwin' или 'aarch64-apple-darwin'"
  '';
}
Используйте код с осторожностью.

Шаг 2. Добавление таргетов в Rust

Активируйте созданное окружение командой:
bash
nix-shell
Используйте код с осторожностью.
NixOS автоматически загрузит нужные LLVM-компиляторы и Clang, настроенные на генерацию кода для macOS. Внутри этого шелла добавьте целевые архитектуры в ваш Rust:
bash
rustup target add x86_64-apple-darwin
rustup target add aarch64-apple-darwin
Используйте код с осторожностью.

Шаг 3. Сборка проекта

Теперь вы можете компилировать приложение под любую архитектуру macOS прямо из этого терминала.
Для Apple Silicon (M1/M2/M3/M4):
bash
cargo build --target aarch64-apple-darwin --release
Используйте код с осторожностью.
Для старых Mac на процессорах Intel:
bash
cargo build --target x86_64-apple-darwin --release
Используйте код с осторожностью.
Готовые бинарники будут лежать в папках target/aarch64-apple-darwin/release/ и target/x86_64-apple-darwin/release/ соответственно.

💡 Важное замечание по NixOS и внешним C-зависимостям (C-bindings)

Если ваш Rust-проект использует библиотеки, которые компилируют под капотом C-код (например, через cc-rs или скрипты build.rs), им также может потребоваться указать правильный C-компилятор.
Если сборка падает с ошибкой компиляции C-файлов, просто добавьте перед командой сборки нужную переменную CC:
bash
# Для Apple Silicon
CC=${mSiliconPkgs.stdenv.cc}/bin/aarch64-apple-darwin-clang cargo build --target aarch64-apple-darwin --release

# Для Intel
CC=${intelPkgs.stdenv.cc}/bin/x86_64-apple-darwin-clang cargo build --target x86_64-apple-darwin --release
Используйте код с осторожностью.

