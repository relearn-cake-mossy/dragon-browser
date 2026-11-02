# Dragon Browser

Dragon Browser là nguyên mẫu trình duyệt tối giản viết bằng Rust. Phần engine
được tách thành thư viện `dragon-core`; mục tiêu ban đầu là đọc báo và tiết kiệm
tài nguyên, nhưng engine đang được phát triển theo từng giai đoạn.

## Chạy cửa sổ trình duyệt

```sh
cargo run --release
cargo run --release -- https://vnexpress.net
```

Ứng dụng mở cửa sổ desktop có thanh địa chỉ và tải nội dung không chặn giao diện.
Đây hiện là **chế độ đọc thử nghiệm**: Dragon trích xuất nội dung HTML và áp dụng
một phần CSS cơ bản từ thẻ `<style>` hoặc thuộc tính `style`: màu chữ, cỡ chữ
`px`, độ đậm, cùng bộ chọn thẻ/class/id đơn giản. CSS layout, selector phức tạp,
stylesheet bên ngoài, JavaScript trên trang, ảnh và Web APIs chưa được hỗ trợ.
Vì vậy VnExpress và các trang hiện đại vẫn chưa hiển thị đúng.

Mỗi tài liệu bị giới hạn ở 8 MiB và các kết nối có timeout. Những giới hạn này
giúp tránh tải trang không giới hạn, nhưng chưa phải cam kết mức RAM thực tế.
Trên Linux, cần thư viện hệ thống cho đồ họa X11 hoặc Wayland.

## Build cho Windows

Trên Windows với Rust stable đã cài:

```powershell
cargo build --release
```

Workflow GitHub Actions chạy test trên `windows-latest`.
