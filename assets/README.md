# Assets Directory

This directory contains static assets used by the application.

## Font Files (Optional)

To enable the `embed_fonts` feature and include Inter fonts in the binary:

1. Download the Inter font family from [Google Fonts](https://fonts.google.com/specimen/Inter):
   - `Inter-Regular.ttf` (Regular weight)
   - `Inter-Bold.ttf` (Bold weight)

2. Place both `.ttf` files in this directory

3. Build with the feature enabled:
   ```bash
   cargo build --release --features embed_fonts
   ```

Without these files, the application will use system default fonts (feature disabled by default).

### Font License

Inter is licensed under the Open Font License (OFL). See the license files included with the fonts.
