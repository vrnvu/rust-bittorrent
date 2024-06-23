# rust-bittorrent

A minimal BitTorrent client written in Rust, focusing on simplicity and functionality.

## Features

- **Torrent Parsing**: Parses .torrent files.
- **Peer Communication**: Connects to peers for file exchange.
- **Asynchronous IO**: Utilizes Tokio for efficient networking.

## Usage

### Command-line Interface (CLI)

```sh
rust-bittorrent --file <path_to_torrent_file> --output_path <output_directory> [--verbose]
```

### Examples

```sh
# Download a torrent file with default logging and output path
rust-bittorrent --file sample.torrent --output_path test.txt

# Download a torrent file with default logging and output path creating a folder
rust-bittorrent --file sample.torrent --output_path test_folder/test.txt

# Download a torrent file with verbose logging
rust-bittorrent --file sample.torrent --output_path test_folder/test.txt --verbose
```