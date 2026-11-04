# Dragon Browser

Dragon Browser là trình duyệt tối giản viết bằng Rust với renderer HTML tự viết
bằng `egui`. Renderer giữ cây lồng nhau và vẽ phần tử block/inline, chữ định
dạng và ảnh bằng giao diện native; không nhúng WebView.

## Chạy cửa sổ trình duyệt

```sh
cargo run --release
cargo run --release -- https://vnexpress.net
```

Thanh tab phía trên cho phép mở tab mới bằng `+`, chuyển tab bằng cách chọn
tiêu đề, và đóng tab bằng `×`. Mỗi tab giữ địa chỉ, nội dung tải và lỗi riêng.
Liên kết văn bản trong trang có thể bấm; URL tương đối được phân giải theo địa
chỉ trang hiện tại và chỉ điều hướng đến HTTP/HTTPS.

Binary headless tùy chọn để kiểm tra bộ parser/renderer thử nghiệm của Dragon
(không mở cửa sổ):

```sh

cargo run --release --features headless --bin dragon-browser-headless -- https://vnexpress.net
```

Renderer hỗ trợ luồng block/inline, selector CSS tag/class/id và selector con/
hậu duệ, màu/chữ, display cơ bản (block/inline/flex), flex row/column với gap,
width/max-width theo px/% và margin/padding px. Chưa có layout CSS đầy đủ
(flex-grow/shrink, grid, position),
bảng hoàn chỉnh, SVG inline hay video; ảnh SVG tĩnh được chuyển sang bitmap.
Ứng dụng tải stylesheet ngoài và tối đa 8 ảnh (kể cả `data-src` phổ biến).
Boa chạy tối đa 64 script JavaScript cổ điển inline/
ngoài theo thứ tự tài liệu; script ngoài tối đa 512 KiB mỗi file.
Script quảng cáo từ một số mạng phổ biến được bỏ qua vì engine chưa có API đấu giá/
đặt quảng cáo.
`document.write/writeln` có thể thêm HTML đơn giản. JavaScript có DOM giả lập ở
mức cơ bản: selector descendant/child, `classList`, thuộc tính DOM, clone node,
`textContent`/`innerHTML`, sửa inline `style`; `navigator`, `location`, `console`,
timer và event listener vẫn là shim đồng bộ/tối giản. DOM chuẩn, event loop,
phần lớn Web APIs, module và script bất đồng bộ chưa được hỗ trợ. Đây **chưa phải
Chromium 30 hoặc trình duyệt tương thích đầy đủ**; trang phức tạp vẫn có thể lệch.
Các trường text/password, textarea, checkbox, radio và select có thể thao tác;
giá trị được giữ trong tab hiện tại. Nút form chưa gửi dữ liệu lên website.

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
