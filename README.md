# Dragon Browser

Dragon Browser là nguyên mẫu trình duyệt tối giản viết bằng Rust. Phần engine
được tách thành thư viện `dragon-core`; mục tiêu ban đầu là đọc báo và tiết kiệm
tài nguyên, nhưng engine đang được phát triển theo từng giai đoạn.

## Chạy cửa sổ trình duyệt

```sh
cargo run --release
cargo run --release -- https://vnexpress.net
```

Ứng dụng dựng phần thân HTML thành các khối nội dung, tải stylesheet ngoài và
tối đa 8 ảnh (kể cả `data-src` phổ biến). Boa chạy một số script JavaScript
inline; hỗ trợ `document.write/writeln` để thêm HTML đơn giản. Script ngoài,
DOM/`window`/Web APIs và JavaScript phức tạp chưa được hỗ trợ. CSS còn giới hạn
ở bộ chọn thẻ/class/id đơn giản và một số thuộc tính chữ/màu/ẩn; chưa có CSS
layout, ảnh nền hay video. Đây **chưa phải trình duyệt tương thích đầy đủ** và
VnExpress vẫn có thể hiển thị thiếu hoặc sai.

Script inline được giới hạn số lượng, kích thước và số vòng lặp. Lỗi script
được hiển thị ở phần đầu trang thay vì âm thầm bỏ qua.

HTML giới hạn 8 MiB, stylesheet 512 KiB mỗi file (tối đa 8 file), ảnh 1 MiB mỗi
file (tối đa 8 ảnh); mỗi ảnh còn giới hạn kích thước giải mã. Các kết nối có
timeout. Đây là giới hạn bảo vệ cơ bản, không phải cam kết RAM cụ thể.

Trên Linux, cần thư viện hệ thống cho đồ họa X11 hoặc Wayland.

## Build cho Windows

Trên Windows với Rust stable đã cài:

```powershell
cargo build --release
```

Workflow GitHub Actions chạy test trên `windows-latest`.
