#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <memory>

namespace pubky_swarm::libtorrent {

struct AlertSnapshotFfi;
struct AddTorrentOptionsFfi;
struct BuildInfoFfi;
struct MagnetInfoFfi;
struct ResumeDataFfi;
struct ResumeRequestFfi;
struct SessionConfigFfi;
struct SessionSnapshotFfi;
struct TorrentMutationFfi;

class SessionHandle final {
public:
  struct Impl;

  explicit SessionHandle(std::unique_ptr<Impl> impl) noexcept;
  ~SessionHandle() noexcept;

  SessionHandle(const SessionHandle &) = delete;
  SessionHandle &operator=(const SessionHandle &) = delete;

  Impl *impl() noexcept;

private:
  std::unique_ptr<Impl> impl_;
};

BuildInfoFfi build_info() noexcept;
MagnetInfoFfi parse_magnet(rust::String uri) noexcept;
std::unique_ptr<SessionHandle>
create_session(SessionConfigFfi config, rust::String &error) noexcept;
TorrentMutationFfi add_magnet(SessionHandle &session, rust::String uri,
                              AddTorrentOptionsFfi options) noexcept;
TorrentMutationFfi
add_torrent_metainfo(SessionHandle &session, rust::Vec<std::uint8_t> metainfo,
                     AddTorrentOptionsFfi options) noexcept;
TorrentMutationFfi
add_resume_data(SessionHandle &session, rust::Vec<std::uint8_t> resume_data,
                AddTorrentOptionsFfi options) noexcept;
rust::String pause_torrent(SessionHandle &session,
                           std::uint64_t torrent_id) noexcept;
rust::String resume_torrent(SessionHandle &session,
                            std::uint64_t torrent_id) noexcept;
rust::String remove_torrent(SessionHandle &session,
                            std::uint64_t torrent_id) noexcept;
rust::String set_file_priority(SessionHandle &session, std::uint64_t torrent_id,
                               std::uint32_t file_index,
                               std::uint8_t priority) noexcept;
rust::String set_file_priorities(SessionHandle &session,
                                 std::uint64_t torrent_id,
                                 rust::Vec<std::uint8_t> priorities) noexcept;
rust::String force_recheck(SessionHandle &session,
                           std::uint64_t torrent_id) noexcept;
rust::String force_reannounce(SessionHandle &session,
                              std::uint64_t torrent_id) noexcept;
rust::String set_torrent_limits(SessionHandle &session,
                                std::uint64_t torrent_id,
                                std::int32_t download_limit,
                                std::int32_t upload_limit) noexcept;
rust::String set_global_limits(SessionHandle &session,
                               std::int32_t download_limit,
                               std::int32_t upload_limit) noexcept;
ResumeRequestFfi save_resume_data(SessionHandle &session,
                                  std::uint64_t torrent_id) noexcept;
ResumeDataFfi poll_resume_data(SessionHandle &session,
                               std::uint64_t request_id) noexcept;
SessionSnapshotFfi snapshot_session(SessionHandle &session) noexcept;
rust::String shutdown_session(SessionHandle &session) noexcept;

} // namespace pubky_swarm::libtorrent
