# 뿌요뿌요!! 20주년 기념판 (PSP) 한글 패처

PSP용 《뿌요뿌요!! 20주년 기념판》(ぷよぷよ！！, NPJH50492) 일본판 ISO에 한글 패치를 적용하는 Rust 코드입니다. ISO 9660 읽기·쓰기, 원본 식별과 쓰기 범위 검사, 게임 ZIP 아카이브 재패킹, FNT/MTX 텍스트 추출과 재삽입, 메뉴·스토리 한글 글꼴 생성, SNT·GIM·DXT 그래픽 처리, EBOOT 복호화와 [retro-typed-isa](https://github.com/mcpads/retro-typed-isa)의 Allegrex 프로필을 이용한 코드 패치, 설치 데이터 제거와 PARAM.SFO 제목 교체를 제공합니다.

배포용 xdelta 패치와 적용 방법은 [뿌요뿌요 시리즈 한글 번역 프로젝트](https://github.com/mcpads/puyo-puyo-kr-patch/tree/main/psp-puyo20)에서 제공합니다.

## 제공하지 않는 것

이 저장소에는 원본 ISO, 패치를 적용한 ISO, 빌드 명세(`config/`의 대부분), 번역 JSON, 한글 그래픽 PNG와 폰트 파일이 없습니다. `config/`에는 원본 식별(`source.json`), 텍스트 제어 코드 표(`text-controls.json`), PRX 복호화 의존성 식별(`prx-decryptor.json`)만 들어 있습니다. 따라서 이 저장소만으로는 배포 패치를 다시 만들 수 없습니다. 아래 입력을 직접 갖춘 경우에만 `build`가 ISO를 생성합니다.

## 빌드와 테스트

```bash
cargo build --locked --release
cargo test
```

기본 테스트는 합성 입력만 사용합니다. 폰트가 필요한 테스트는 `#[ignore = "requires ..."]`로 필요한 파일을 밝혀 두었습니다. 해당 폰트를 아래 경로에 둔 뒤 `cargo test -- --ignored`로 실행하며, 파일이 없으면 성공으로 넘어가지 않고 실패합니다.

## 지원 원본

| 원본 | 크기 | SHA-256 |
| --- | --- | --- |
| 일본판 (NPJH50492, v1.00) | 1,353,154,560바이트 | `8f26636c1bc2b473bf1e54eec0ca798273b982481924520b3678bc899c0f0aa2` |

`verify-source`로 원본을 확인할 수 있으며, `build`도 시작할 때 크기와 SHA-256을 검사합니다.

```bash
target/release/puyo20-tool verify-source path/to/japanese.iso
```

## 빌드 입력

`build`는 저장소 루트의 `config/`와 `assets/`를 읽습니다. 아래 파일을 저장소 루트에 배치하고 루트에서 실행합니다. 두 경로는 `.gitignore`로 커밋에서 제외됩니다(`config/`에 동봉한 세 파일은 예외).

| 입력 | 경로 | 비고 |
| --- | --- | --- |
| 빌드 명세 | `config/*.json`, `config/graphics/`, `config/graphics-build/` | 번역 파일·그래픽·폰트의 경로와 SHA-256을 고정합니다. 빌드 시작 시 모든 해시를 검사합니다. |
| 번역 JSON | `assets/translation/` | 메뉴·학교·상점·스토리 문장 |
| 한글 그래픽 | `assets/graphics/` | 채택한 PNG와 원본 대비 편집 범위 |
| 메뉴·학교·승리 문장 폰트 | `assets/fonts/poc/Galmuri11.ttf` | [Galmuri](https://github.com/quiple/galmuri) |
| 스토리 대사 폰트 | `assets/fonts/private/MaplestoryLight.ttf` | [메이플스토리 서체](https://maplestory.nexon.com/Media/Font) Light |
| PPSSPP 소스 | `--ppsspp-root`로 지정 | [hrydgard/ppsspp](https://github.com/hrydgard/ppsspp) 커밋 `56c694d88bbf82270e8b472fe63abd60f3f8e0a9` |

폰트는 재배포 조건을 이 저장소에서 보장할 수 없어 포함하지 않습니다. 각 폰트의 라이선스는 배포처에서 확인하세요. 배포 패치 v1.0.0은 다음 폰트 파일로 만들었습니다. 빌드 명세가 이 해시를 고정하므로 다른 파일로는 진행하지 않습니다.

```text
2c709890595668f7bdb6df408420fda957dde0288e95b31a1cc17a2ab98b4b4f  Galmuri11.ttf
6d51d8e576f77b01914095aa1f69f9d37c16d93fe940d748962867f218442ba9  MaplestoryLight.ttf
```

EBOOT는 암호화되어 있어 빌드 중에 복호화합니다. `build`는 `--ppsspp-root` 아래의 PPSSPP 복호화 소스(`config/prx-decryptor.json`에 나열한 파일)를 SHA-256으로 확인한 뒤 임시 디렉터리에서 `cc`·`c++`로 컴파일해 실행합니다. PPSSPP 소스는 이 저장소에 포함하지 않습니다.

## ISO 생성

```bash
target/release/puyo20-tool build path/to/japanese.iso \
  --ppsspp-root path/to/ppsspp --output out/ko
```

출력 디렉터리(새 경로여야 합니다)에 `menu.iso`와 `report.json`이 생깁니다. 빌드는 원본에서 설치 데이터(`/PSP_GAME/INSDIR`)를 제거하므로 결과 ISO는 원본보다 작습니다. 배포 패치 v1.0.0을 적용한 ISO는 906,461,184바이트, SHA-256 `cbc9c2e5ecc764178e49a257baf8c5a9859e7b16600a7753bab0249e8f4960a2`입니다. 다음 명령으로 만든 xdelta는 배포 패치 v1.0.0(8,479,446바이트, SHA-256 `2a0e2bec92e9c4605ab014076161cc807675b7f1b4a5b337c49e4e63b8a4580d`)과 같습니다.

```bash
xdelta3 -9 -e -s path/to/japanese.iso out/ko/menu.iso puyo20-ko.xdelta
```

## 그 밖의 명령

텍스트 추출(`extract-text`), 메뉴 추출·합성(`extract-menu`, `compose`, `reinsert-menu`), 그래픽 준비(`prepare-region`, `prepare-labels`, `prepare-gim`, `prepare-dxt`, `prepare-panels`, `render-display`), 해시 고정 검사(`pins`) 등의 사용법은 `cargo run -- help <명령>`으로 확인할 수 있습니다. 대부분은 위 빌드 입력 중 일부를 요구합니다.

## 라이선스

이 저장소의 소스 코드는 [MIT License](LICENSE)로 제공합니다.
