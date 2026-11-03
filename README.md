# Dragon Browser

Dragon Browser là trình duyệt tối giản viết bằng Rust với renderer HTML tự viết
bằng `egui`. Renderer giữ cây lồng nhau và vẽ phần tử block/inline, chữ định
dạng và ảnh bằng giao diện native; không nhúng WebView.

## Chạy cửa sổ trình duyệt

```sh
cargo run --release
cargo run --release -- https://vnexpress.net
```

Binary headless tùy chọn để kiểm tra bộ parser/renderer thử nghiệm của Dragon
(không mở cửa sổ):

```sh

cargo run --release --features headless --bin dragon-browser-headless -- https://vnexpress.net
```

Renderer hỗ trợ luồng block/inline và một tập nhỏ kiểu chữ/màu CSS; chưa có
layout CSS đầy đủ (flex/grid/position), bảng hoàn chỉnh, SVG hay video. Ứng dụng
tải stylesheet ngoài và tối đa 8 ảnh (kể cả `data-src` phổ biến). Boa chạy tối đa 16 script JavaScript
cổ điển inline/ngoài theo thứ tự tài liệu, tối đa 32 script; script ngoài tối
đa 512 KiB mỗi file.
`document.write/writeln` có thể thêm HTML đơn giản. JavaScript có DOM giả lập ở
mức cơ bản: truy vấn tag/id/class đơn giản, tạo/thêm/xóa node, `textContent` và
`innerHTML`; `navigator`, `location`, `console`, timer và event listener là các
shim đồng bộ/tối giản. DOM thật, event loop, phần lớn Web APIs, module, script
bất đồng bộ và JavaScript tương tác phức tạp chưa được hỗ trợ. Đây là bước tương
thích nhỏ, không phải Chromium 30.
CSS còn giới hạn ở bộ chọn thẻ/class/id đơn giản và một số thuộc tính chữ/màu/ẩn;
chưa có CSS layout, ảnh nền hay video. Đây **chưa phải Chromium 30 hoặc trình
duyệt tương thích đầy đủ**; VnExpress hiện tại vẫn có thể hiển thị thiếu hoặc sai.

Script được giới hạn số lượng, kích thước và số vòng lặp. Lỗi tải/chạy script
được hiển thị ở phần đầu trang thay vì âm thầm bỏ qua.

HTML giới hạn 8 MiB, stylesheet 512 KiB mỗi file (tối đa 8 file), ảnh 1 MiB mỗi
file (tối đa 8 ảnh); mỗi ảnh còn giới hạn kích thước giải mã. Các kết nối có
timeout. Đây là giới hạn bảo vệ cơ bản, không ph
ải cam kết RAM cụ thể.

Trên Linux, cần thư viện hệ thống cho đồ họa X11 hoặc Wayland.

## Build cho Windows

Trên Windows với Rust stable đã cài:

```powershell
cargo build --release
```

Workflow GitHub Actions chạy test trên `windows-latest`.
