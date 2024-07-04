# rust-bittorrent

A minimal BitTorrent client and tracker written in Rust, focusing on simplicity and functionality.

## Project Structure

- **bt**: The BitTorrent client that handles torrent parsing, peer communication, and file exchange.
- **tracker**: The HTTP server that acts as a tracker for peers to announce themselves.

## Features

### BitTorrent Client (bt)

- **Torrent Parsing**: Parses .torrent files.
- **Peer Communication**: Connects to peers for file exchange.
- **Asynchronous IO**: Utilizes Tokio for efficient networking.

### Tracker (tracker) (TODO/Work in progress)

- **Peer Announcement**: Accepts and manages peer announcements.
- **Peer List Management**: Provides a list of peers to clients.

## Usage

### BitTorrent Client (bt)

#### Command-line Interface (CLI)

```sh
bt --file <path_to_torrent_file> --output_path <output_directory> [--verbose]
```

#### Examples

```sh
# Download a torrent file with default logging and output path
bt --file sample.torrent --output_path test.txt

# Download a torrent file with default logging and output path creating a folder
bt --file sample.torrent --output_path test_folder/test.txt

# Download a torrent file with verbose logging
bt --file sample.torrent --output_path test_folder/test.txt --verbose
```

### Tracker

#### Starting the Tracker Server

The tracker runs an HTTP server on `http://127.0.0.1:3030` by default.

```sh
cargo run -p tracker
```

#### Example Announcement Request

Peers announce themselves to the tracker by sending a GET request to the `/announce` endpoint with the required query parameters.

Example using `curl`:

```sh
curl "http://127.0.0.1:3030/announce?info_hash=12345&peer_id=peer1&ip=192.168.1.2&port=6881"
```

## Running the Project

### Workspace Setup

Ensure you have Rust installed. Clone the repository and navigate to the project directory.

```sh
git clone https://github.com/yourusername/rust-bittorrent.git
cd rust-bittorrent
```

### Building the Project

To build the entire workspace:

```sh
cargo build
```

### Running the BitTorrent Client

Navigate to the `bt` directory and run the client:

```sh
cd bt
cargo run -- --file <path_to_torrent_file> --output_path <output_directory> [--verbose]
```

or

```sh
cargo run --bin bt --file <path_to_torrent_file> --output_path <output_directory> [--verbose]
```

### Running the Tracker

Navigate to the `tracker` directory and start the server:

```sh
cd tracker
cargo run
```

or

```sh
cargo run --bin tracker
```

#### Example Announcement Request

Peers announce themselves to the tracker by sending a GET request to the `/announce` endpoint with the required query parameters.

Example using `curl`:

```sh
curl -i "http://127.0.0.1:3030/announce?info_hash=12345&peer_id=peer1&ip=192.168.1.2&port=6881"
```