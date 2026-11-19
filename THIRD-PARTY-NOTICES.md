# Third-party notices

Sidescreen is licensed under the GNU General Public License v3.0 (see [LICENSE](LICENSE)).
It includes or works with the following projects.

## Included in Sidescreen

| Project | Used for | License |
|---|---|---|
| [Lan Mouse](https://github.com/feschber/lan-mouse): `input-capture`, `input-emulation`, `input-event` (in `third_party/lan-mouse`, with small changes marked "Sidescreen:") | Capturing the PC's mouse and keyboard, replaying them on the Mac | GPL-3.0-or-later |
| [Tauri](https://tauri.app) and its plugins | The app on both computers | MIT or Apache-2.0 |
| [arboard](https://github.com/1Password/arboard) | Reading and writing the clipboard | MIT or Apache-2.0 |
| [tokio](https://tokio.rs), [serde](https://serde.rs), [RustCrypto](https://github.com/RustCrypto) (`chacha20poly1305`, `hkdf`, `sha2`, `hmac`), [mdns-sd](https://github.com/keepsimple1/mdns-sd), [reqwest](https://github.com/seanmonstar/reqwest), [png](https://github.com/image-rs/image-png), [if-addrs](https://github.com/messense/if-addrs), [windows-rs](https://github.com/microsoft/windows-rs), [core-graphics / core-foundation](https://github.com/servo/core-foundation-rs) | Networking, encryption, discovery, system access | MIT and/or Apache-2.0 |

Each crate's full license is in its source, listed in `Cargo.lock`.

## Installed alongside, not included

| Project | Used for | License |
|---|---|---|
| [Moonlight](https://github.com/moonlight-stream/moonlight-qt) | Showing the Mac's display on the PC (installed by the Windows installer via winget) | GPL-3.0 |
| [Sunshine](https://github.com/LizardByte/Sunshine) | Streaming the Mac's display (installed from the Mac setup guide via Homebrew) | GPL-3.0 |

## Thanks

The Mac's virtual display uses CoreGraphics' virtual display API the way [DeskPad](https://github.com/Stengo/DeskPad) (MIT) showed.
