# Dragon Browser

Dragon Browser là nguyên mẫu trình duyệt tối giản viết bằng Rust. Phần engine
được tách thành thư viện `dragon-core`; mục tiêu ban đầu là đọc báo và tiết kiệm
tài nguyên, nhưng engine đang được phát triển theo từng giai đoạn.

## Chạy thử

```sh
cargo run --release -- https://example.com
```

Hiện tại ứng dụng tải HTML và trích xuất văn bản, ưu tiên `<article>` hoặc
`<main>`. `dragon-core` có thể chạy JavaScript cơ bản qua Boa, nhưng runtime chưa
được nối với trang: chưa có `window`, DOM, `document`, CSS layout, ảnh hay Web APIs.
Vì vậy **chưa thể hiển thị hoặc chạy VnExpress đúng cách**.

Mỗi tài liệu bị giới hạn ở 8 MiB và các kết nối có timeout. Những giới hạn này
giúp tránh tải trang không giới hạn, nhưng chưa phải cam kết mức RAM thực tế.

## Build cho Windows

Trên Windows với Rust stable đã cài:

```powershell
cargo build --release
```

Workflow GitHub Actions chạy test trên `windows-latest`.
