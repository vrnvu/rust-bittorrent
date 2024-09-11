# rust-bittorrent

A minimal BitTorrent client and tracker written in Rust, focusing on simplicity and functionality.

## Project Structure

- **bt**: The BitTorrent client that handles torrent parsing, peer communication, and file exchange.
- **tracker**: The HTTP server that acts as a tracker for peers to announce themselves.
- **models**: Shared data models used by both the client and tracker.

## Features

### BitTorrent Client (bt)

- **Torrent Parsing**: Parses .torrent files.
- **Peer Communication**: Connects to peers for file exchange.
- **Asynchronous IO**: Utilizes Tokio for efficient networking.
- **Upload Mode**: Allows sharing of files with other peers.
- **Download Mode**: Fetches files from other peers.
- **Parallel Download**: Downloads files from multiple peers simultaneously, improving download speeds.

### Tracker

- **Peer Announcement**: Accepts and manages peer announcements.
- **Peer List Management**: Provides a list of peers to clients.
- **Content Discovery**: Helps peers discover what content is available in the network.

## Current State of the Project

The project currently supports basic BitTorrent functionality with enhancements for parallel downloads:

1. The tracker allows peers to register the content they have available.
2. Peers can operate in both upload and download modes.
3. The tracker is used for peer discovery and content discovery.
4. The client can parse .torrent files and communicate with peers.
5. The client can download files from multiple peers simultaneously, improving download speeds.

## Usage and Explanation

### 1. Tracker Setup

1. Start the tracker on port 9999 (currently hardcoded):
   ```
   tracker 9999
   ```
2. The tracker initializes and listens for HTTP requests from peers.

### 2. File Upload Process

1. Run the upload command:
   ```
   bt upload --file <path_to_file> [--verbose]
   ```
2. The client reads the file and generates an info hash.
3. The client sends an HTTP announcement to the tracker with the info hash and peer information.
4. The tracker stores this information (no health check implemented yet).
5. The upload process currently blocks the peer process (background operation planned for future).

### 3. File Download Process

1. Run the download command:
   ```
   bt download --file <path_to_torrent_file> --output_path <output_directory> [--verbose]
   ```
2. The client parses the .torrent file to extract the info hash and tracker URL.
3. The client sends an HTTP request to the tracker to get a list of peers for the desired file.
4. The tracker responds with available peers for the requested info hash.
5. The client initiates the BitTorrent protocol with the available peers in parallel:
   - Establishes connections
   - Performs handshakes
   - Exchanges piece information
   - Requests and receives file pieces
6. The client assembles the received pieces and saves the complete file to the specified output path.

### Important Notes

- The current implementation only supports HTTP trackers.
- The BitTorrent protocol implementation is functional and compatible with other peers/trackers but lacks advanced optimizations.
- The upload process currently doesn't implement background seeding (planned for future updates).
- The system doesn't currently implement peer health checks or sophisticated peer selection strategies.
- The client now supports parallel downloads from multiple peers, enhancing download speeds.

### Future Improvements

- Implement background seeding for uploads
- Add health checks for peers
- Implement more sophisticated peer selection and piece selection algorithms
- Support for other tracker protocols (e.g., UDP)
- Implement DHT (Distributed Hash Table) for trackerless operation
- Optimize parallel download for better performance and resource utilization
