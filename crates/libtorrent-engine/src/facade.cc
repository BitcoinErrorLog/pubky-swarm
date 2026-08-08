#include "libtorrent-engine-sys/src/ffi.rs.h"

#include "facade.hpp"
#include "libtorrent/alert.hpp"
#include "libtorrent/alert_types.hpp"
#include "libtorrent/config.hpp"
#include "libtorrent/download_priority.hpp"
#include "libtorrent/load_torrent.hpp"
#include "libtorrent/magnet_uri.hpp"
#include "libtorrent/read_resume_data.hpp"
#include "libtorrent/session.hpp"
#include "libtorrent/settings_pack.hpp"
#include "libtorrent/torrent_flags.hpp"
#include "libtorrent/torrent_handle.hpp"
#include "libtorrent/torrent_info.hpp"
#include "libtorrent/version.hpp"
#include "libtorrent/write_resume_data.hpp"

#include <algorithm>
#include <cstdint>
#include <exception>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace pubky_swarm::libtorrent {
namespace lt = ::libtorrent;

namespace {

constexpr std::size_t max_magnet_uri_bytes = 32 * 1024;
constexpr std::size_t max_torrent_bytes = 8 * 1024 * 1024;
constexpr std::size_t max_resume_bytes = 16 * 1024 * 1024;
constexpr std::size_t max_save_path_bytes = 4096;
constexpr std::size_t max_file_count = 100000;
constexpr std::size_t max_alert_backlog = 4096;

rust::String rust_string(std::string_view value) {
  return rust::String(value.data(), value.size());
}

std::string owned_string(const rust::String &value) {
  return std::string(value.data(), value.size());
}

template <typename Hash> std::string hex_hash(const Hash &hash) {
  constexpr char digits[] = "0123456789abcdef";
  std::string output;
  output.resize(static_cast<std::size_t>(Hash::size()) * 2);
  for (std::ptrdiff_t index = 0; index < Hash::size(); ++index) {
    const auto byte =
        static_cast<unsigned char>(hash.data()[static_cast<std::size_t>(index)]);
    output[static_cast<std::size_t>(index) * 2] = digits[byte >> 4];
    output[static_cast<std::size_t>(index) * 2 + 1] = digits[byte & 0x0f];
  }
  return output;
}

rust::String exception_message(std::string_view operation,
                               const std::exception &error) {
  std::string message(operation);
  message.append(": ");
  message.append(error.what());
  return rust_string(message);
}

rust::String unknown_exception_message(std::string_view operation) {
  std::string message(operation);
  message.append(": unknown C++ exception");
  return rust_string(message);
}

void stop_session(std::unique_ptr<lt::session> &session) {
  if (!session) {
    return;
  }
  auto proxy = session->abort();
  session.reset();
}

void validate_string(std::string_view value, std::size_t maximum,
                     std::string_view label, bool allow_empty = false) {
  if (!allow_empty && value.empty()) {
    throw std::invalid_argument(std::string(label) + " must not be empty");
  }
  if (value.size() > maximum) {
    throw std::invalid_argument(std::string(label) + " exceeds the size limit");
  }
  if (value.find('\0') != std::string_view::npos) {
    throw std::invalid_argument(std::string(label) +
                                " contains an embedded NUL byte");
  }
}

void validate_bytes(std::size_t size, std::size_t maximum,
                    std::string_view label) {
  if (size == 0) {
    throw std::invalid_argument(std::string(label) + " must not be empty");
  }
  if (size > maximum) {
    throw std::invalid_argument(std::string(label) + " exceeds the size limit");
  }
}

void validate_priority(std::uint8_t priority) {
  if (priority > 7) {
    throw std::invalid_argument("file priority must be in the range 0..=7");
  }
}

void validate_rate_limit(std::int32_t limit) {
  if (limit < -1 || limit == 0) {
    throw std::invalid_argument(
        "rate limit must be -1 (unlimited) or positive");
  }
}

lt::load_torrent_limits loading_limits(std::size_t maximum) {
  lt::load_torrent_limits limits;
  limits.max_buffer_size = static_cast<int>(maximum);
  limits.max_pieces = 1024 * 1024;
  limits.max_decode_depth = 100;
  limits.max_decode_tokens = 2 * 1024 * 1024;
  return limits;
}

lt::span<char const> byte_span(const rust::Vec<std::uint8_t> &bytes) {
  return {reinterpret_cast<char const *>(bytes.data()),
          static_cast<std::ptrdiff_t>(bytes.size())};
}

template <typename Flag>
void assign_flag(lt::torrent_flags_t &flags, Flag flag, bool enabled) {
  if (enabled) {
    flags |= flag;
  } else {
    flags &= ~flag;
  }
}

void apply_add_options(lt::add_torrent_params &parameters,
                       const AddTorrentOptionsFfi &options) {
  const auto save_path = owned_string(options.save_path);
  validate_string(save_path, max_save_path_bytes, "save path");
  validate_rate_limit(options.download_limit);
  validate_rate_limit(options.upload_limit);
  if (options.file_priorities.size() > max_file_count) {
    throw std::invalid_argument("file priority list exceeds the file limit");
  }

  parameters.save_path = save_path;
  parameters.download_limit = options.download_limit;
  parameters.upload_limit = options.upload_limit;
  parameters.file_priorities.clear();
  parameters.file_priorities.reserve(options.file_priorities.size());
  for (const auto priority : options.file_priorities) {
    validate_priority(priority);
    parameters.file_priorities.emplace_back(lt::download_priority_t{priority});
  }

  assign_flag(parameters.flags, lt::torrent_flags::paused,
              options.flags.paused);
  assign_flag(parameters.flags, lt::torrent_flags::auto_managed,
              options.flags.auto_managed);
  assign_flag(parameters.flags, lt::torrent_flags::seed_mode,
              options.flags.seed_mode);
  assign_flag(parameters.flags, lt::torrent_flags::upload_mode,
              options.flags.upload_mode);
  assign_flag(parameters.flags, lt::torrent_flags::share_mode,
              options.flags.share_mode);
  assign_flag(parameters.flags, lt::torrent_flags::sequential_download,
              options.flags.sequential_download);
  assign_flag(parameters.flags, lt::torrent_flags::stop_when_ready,
              options.flags.stop_when_ready);
  assign_flag(parameters.flags, lt::torrent_flags::duplicate_is_error,
              options.flags.duplicate_is_error);
  assign_flag(parameters.flags, lt::torrent_flags::default_dont_download,
              options.flags.default_dont_download);
  parameters.flags |= lt::torrent_flags::disable_dht;
  parameters.flags |= lt::torrent_flags::disable_lsd;
  parameters.flags |= lt::torrent_flags::disable_pex;
}

} // namespace

struct SessionHandle::Impl {
  struct TorrentRecord {
    std::uint64_t id;
    lt::torrent_handle handle;
    bool active;
  };

  struct ResumeRecord {
    std::uint64_t id;
    lt::torrent_handle handle;
    std::uint8_t state;
    std::vector<std::uint8_t> bytes;
    std::string error;
  };

  explicit Impl(std::unique_ptr<lt::session> value)
      : session(std::move(value)) {}

  std::unique_ptr<lt::session> session;
  std::uint64_t next_torrent_id = 1;
  std::uint64_t next_resume_request_id = 1;
  std::vector<TorrentRecord> torrents;
  std::vector<ResumeRecord> resume_requests;
  std::vector<AlertSnapshotFfi> alert_backlog;
};

SessionHandle::SessionHandle(std::unique_ptr<Impl> impl) noexcept
    : impl_(std::move(impl)) {}

SessionHandle::Impl *SessionHandle::impl() noexcept { return impl_.get(); }

SessionHandle::~SessionHandle() noexcept {
  try {
    if (impl_) {
      stop_session(impl_->session);
    }
  } catch (...) {
    // Explicit close reports shutdown errors. Drop has no error channel.
  }
}

BuildInfoFfi build_info() noexcept {
  BuildInfoFfi result{};
  try {
    result.version = rust_string(lt::version());
    result.revision = rust_string(LIBTORRENT_REVISION);
    result.abi_version = TORRENT_ABI_VERSION;
#if PUBKY_LIBTORRENT_STATIC
    result.flags.emplace_back("static-link=1");
#else
    result.flags.emplace_back("static-link=0");
#endif
#ifdef TORRENT_USE_OPENSSL
    result.flags.emplace_back("openssl=1");
#else
    result.flags.emplace_back("openssl=0");
#endif
#ifdef TORRENT_DISABLE_DHT
    result.flags.emplace_back("dht=0");
#else
    result.flags.emplace_back("dht=1");
#endif
#ifdef TORRENT_DISABLE_EXTENSIONS
    result.flags.emplace_back("extensions=0");
#else
    result.flags.emplace_back("extensions=1");
#endif
#ifdef TORRENT_DISABLE_LOGGING
    result.flags.emplace_back("logging=0");
#else
    result.flags.emplace_back("logging=1");
#endif
#if TORRENT_USE_I2P
    result.flags.emplace_back("i2p=1");
#else
    result.flags.emplace_back("i2p=0");
#endif
#ifdef TORRENT_NO_DEPRECATE
    result.flags.emplace_back("deprecated-functions=0");
#else
    result.flags.emplace_back("deprecated-functions=1");
#endif
#ifdef BOOST_NO_EXCEPTIONS
    result.flags.emplace_back("exceptions=0");
#else
    result.flags.emplace_back("exceptions=1");
#endif
  } catch (const std::exception &error) {
    result.error =
        exception_message("reading libtorrent build information", error);
  } catch (...) {
    result.error =
        unknown_exception_message("reading libtorrent build information");
  }
  return result;
}

MagnetInfoFfi parse_magnet(rust::String uri) noexcept {
  MagnetInfoFfi result{};
  try {
    const auto owned_uri = owned_string(uri);
    validate_string(owned_uri, max_magnet_uri_bytes, "magnet URI");
    lt::error_code error;
    auto parameters = lt::parse_magnet_uri(owned_uri, error);
    if (error) {
      result.error = rust_string(error.message());
      return result;
    }
    if (parameters.info_hashes.has_v1()) {
      result.v1_hash = rust_string(hex_hash(parameters.info_hashes.v1));
      result.has_v1 = true;
    }
    if (parameters.info_hashes.has_v2()) {
      result.v2_hash = rust_string(hex_hash(parameters.info_hashes.v2));
      result.has_v2 = true;
    }
    if (!parameters.name.empty()) {
      result.name = rust_string(parameters.name);
      result.has_name = true;
    }
    result.trackers.reserve(parameters.trackers.size());
    for (const auto &tracker : parameters.trackers) {
      result.trackers.emplace_back(rust_string(tracker));
    }
  } catch (const std::exception &error) {
    result.error = exception_message("parsing magnet URI", error);
  } catch (...) {
    result.error = unknown_exception_message("parsing magnet URI");
  }
  return result;
}

std::unique_ptr<SessionHandle>
create_session(SessionConfigFfi config, rust::String &error) noexcept {
  try {
    const auto user_agent = owned_string(config.user_agent);
    const auto listen_interfaces = owned_string(config.listen_interfaces);
    validate_string(user_agent, 1024, "user agent");
    validate_string(listen_interfaces, 1024, "listen interfaces");
    lt::settings_pack settings;
    settings.set_str(lt::settings_pack::user_agent, user_agent);
    settings.set_str(lt::settings_pack::listen_interfaces, listen_interfaces);
    settings.set_bool(lt::settings_pack::enable_dht, config.enable_dht);
    settings.set_bool(lt::settings_pack::enable_lsd, config.enable_lsd);
    settings.set_bool(lt::settings_pack::enable_upnp, config.enable_upnp);
    settings.set_bool(lt::settings_pack::enable_natpmp, config.enable_natpmp);
    settings.set_bool(lt::settings_pack::enable_outgoing_tcp,
                      config.enable_outgoing_tcp);
    settings.set_bool(lt::settings_pack::enable_incoming_tcp,
                      config.enable_incoming_tcp);
    settings.set_bool(lt::settings_pack::enable_outgoing_utp,
                      config.enable_outgoing_utp);
    settings.set_bool(lt::settings_pack::enable_incoming_utp,
                      config.enable_incoming_utp);
    settings.set_int(lt::settings_pack::alert_mask,
                     static_cast<std::int32_t>(config.alert_mask));
    auto native_session = std::make_unique<lt::session>(std::move(settings));
    auto implementation =
        std::make_unique<SessionHandle::Impl>(std::move(native_session));
    return std::make_unique<SessionHandle>(std::move(implementation));
  } catch (const std::exception &exception) {
    error = exception_message("creating libtorrent session", exception);
  } catch (...) {
    error = unknown_exception_message("creating libtorrent session");
  }
  return nullptr;
}

namespace {

SessionHandle::Impl &require_impl(SessionHandle &handle) {
  auto *implementation = handle.impl();
  if (implementation == nullptr || !implementation->session) {
    throw std::runtime_error("libtorrent session is closed");
  }
  return *implementation;
}

SessionHandle::Impl::TorrentRecord &
find_torrent(SessionHandle::Impl &implementation, std::uint64_t torrent_id) {
  const auto record = std::find_if(
      implementation.torrents.begin(), implementation.torrents.end(),
      [torrent_id](const auto &candidate) {
        return candidate.id == torrent_id && candidate.active;
      });
  if (record == implementation.torrents.end()) {
    throw std::invalid_argument("unknown or removed torrent ID");
  }
  return *record;
}

std::uint64_t torrent_id_for_handle(const SessionHandle::Impl &implementation,
                                    const lt::torrent_handle &handle) {
  const auto record = std::find_if(
      implementation.torrents.begin(), implementation.torrents.end(),
      [&handle](const auto &candidate) { return candidate.handle == handle; });
  return record == implementation.torrents.end() ? 0 : record->id;
}

TorrentSnapshotFfi
snapshot_torrent(const SessionHandle::Impl::TorrentRecord &record) {
  TorrentSnapshotFfi result{};
  const auto query_flags = lt::torrent_handle::query_name |
                           lt::torrent_handle::query_save_path |
                           lt::torrent_handle::query_torrent_file;
  const auto status = record.handle.status(query_flags);
  const auto hashes = record.handle.info_hashes();
  result.id = record.id;
  if (hashes.has_v1()) {
    result.v1_hash = rust_string(hex_hash(hashes.v1));
    result.has_v1 = true;
  }
  if (hashes.has_v2()) {
    result.v2_hash = rust_string(hex_hash(hashes.v2));
    result.has_v2 = true;
  }
  result.name = rust_string(status.name);
  result.save_path = rust_string(status.save_path);
  result.state = static_cast<std::uint8_t>(status.state);
  result.progress_ppm =
      static_cast<std::uint32_t>((std::max)(status.progress_ppm, 0));
  result.has_metadata = status.has_metadata;
  result.is_paused = bool(status.flags & lt::torrent_flags::paused);
  result.is_auto_managed =
      bool(status.flags & lt::torrent_flags::auto_managed);
  result.is_sequential_download =
      bool(status.flags & lt::torrent_flags::sequential_download);
  result.is_seed_mode = bool(status.flags & lt::torrent_flags::seed_mode);
  result.is_upload_mode = bool(status.flags & lt::torrent_flags::upload_mode);
  result.is_share_mode = bool(status.flags & lt::torrent_flags::share_mode);
  result.is_finished = status.is_finished;
  result.is_seeding = status.is_seeding;
  result.total_bytes = status.total;
  result.wanted_bytes = status.total_wanted;
  result.wanted_done_bytes = status.total_wanted_done;
  result.all_time_download_bytes = status.all_time_download;
  result.all_time_upload_bytes = status.all_time_upload;
  result.download_rate = status.download_rate;
  result.upload_rate = status.upload_rate;
  result.connected_peers = status.num_peers;
  result.connected_seeds = status.num_seeds;
  result.download_limit = record.handle.download_limit();
  result.upload_limit = record.handle.upload_limit();
  if (status.errc) {
    result.has_error = true;
    result.error_message = rust_string(status.errc.message());
  }

  const auto torrent_info = status.torrent_file.lock();
  if (torrent_info) {
    const auto &files = torrent_info->files();
    const auto count = files.num_files();
    if (count < 0 || static_cast<std::size_t>(count) > max_file_count) {
      throw std::runtime_error("torrent file count exceeds the snapshot limit");
    }
    const auto priorities = record.handle.get_file_priorities();
    result.files.reserve(static_cast<std::size_t>(count));
    for (int native_index = 0; native_index < count; ++native_index) {
      const lt::file_index_t index{native_index};
      const auto priority =
          static_cast<std::size_t>(native_index) < priorities.size()
              ? static_cast<std::uint8_t>(
                    priorities[static_cast<std::size_t>(native_index)])
              : static_cast<std::uint8_t>(lt::default_priority);
      FileSnapshotFfi file{};
      file.index = static_cast<std::uint32_t>(native_index);
      file.path = rust_string(files.file_path(index));
      file.size = files.file_size(index);
      file.priority = priority;
      file.is_selected = priority != 0;
      file.is_pad_file = files.pad_file_at(index);
      result.files.emplace_back(std::move(file));
    }
  }
  return result;
}

TorrentMutationFfi add_parameters(SessionHandle &handle,
                                  lt::add_torrent_params parameters) {
  TorrentMutationFfi result{};
  auto &implementation = require_impl(handle);
  lt::error_code error;
  auto native_handle =
      implementation.session->add_torrent(std::move(parameters), error);
  if (error) {
    throw std::runtime_error(error.message());
  }
  const auto existing = std::find_if(
      implementation.torrents.begin(), implementation.torrents.end(),
      [&native_handle](const auto &candidate) {
        return candidate.active && candidate.handle == native_handle;
      });
  if (existing != implementation.torrents.end()) {
    result.torrent = snapshot_torrent(*existing);
    return result;
  }

  implementation.torrents.push_back(
      {implementation.next_torrent_id++, std::move(native_handle), true});
  try {
    result.torrent = snapshot_torrent(implementation.torrents.back());
  } catch (...) {
    implementation.session->remove_torrent(
        implementation.torrents.back().handle);
    implementation.torrents.pop_back();
    throw;
  }
  return result;
}

void append_alert(SessionHandle::Impl &implementation, const lt::alert &alert,
                  std::uint64_t torrent_id) {
  AlertSnapshotFfi snapshot{};
  snapshot.type_id = alert.type();
  snapshot.type_name = rust_string(alert.what());
  snapshot.message = rust_string(alert.message());
  snapshot.category = static_cast<std::uint32_t>(alert.category());
  snapshot.torrent_id = torrent_id;
  snapshot.has_torrent_id = torrent_id != 0;
  if (implementation.alert_backlog.size() == max_alert_backlog) {
    implementation.alert_backlog.erase(
        implementation.alert_backlog.begin());
  }
  implementation.alert_backlog.emplace_back(std::move(snapshot));
}

void complete_resume_request(SessionHandle::Impl &implementation,
                             const lt::save_resume_data_alert &alert) {
  const auto request = std::find_if(
      implementation.resume_requests.begin(),
      implementation.resume_requests.end(), [&alert](const auto &candidate) {
        return candidate.state == 0 && candidate.handle == alert.handle;
      });
  if (request == implementation.resume_requests.end()) {
    return;
  }
  try {
    const auto bytes = lt::write_resume_data_buf(alert.params);
    if (bytes.size() > max_resume_bytes) {
      request->state = 2;
      request->error = "saved resume data exceeds the size limit";
      return;
    }
    request->bytes.assign(bytes.begin(), bytes.end());
    request->state = 1;
  } catch (const std::exception &error) {
    request->state = 2;
    request->error =
        std::string("serializing saved resume data: ") + error.what();
  } catch (...) {
    request->state = 2;
    request->error =
        "serializing saved resume data: unknown C++ exception";
  }
}

void fail_resume_request(SessionHandle::Impl &implementation,
                         const lt::save_resume_data_failed_alert &alert) {
  const auto request = std::find_if(
      implementation.resume_requests.begin(),
      implementation.resume_requests.end(), [&alert](const auto &candidate) {
        return candidate.state == 0 && candidate.handle == alert.handle;
      });
  if (request != implementation.resume_requests.end()) {
    request->state = 2;
    request->error = alert.error.message();
  }
}

void process_alerts(SessionHandle::Impl &implementation) {
  std::vector<lt::alert *> alerts;
  std::vector<std::uint64_t> completed_removals;
  implementation.session->pop_alerts(&alerts);
  for (const auto *alert : alerts) {
    std::uint64_t torrent_id = 0;
    if (const auto *torrent_alert =
            dynamic_cast<const lt::torrent_alert *>(alert)) {
      torrent_id =
          torrent_id_for_handle(implementation, torrent_alert->handle);
    }
    if (const auto *resume =
            lt::alert_cast<lt::save_resume_data_alert>(alert)) {
      complete_resume_request(implementation, *resume);
    } else if (const auto *failed =
                   lt::alert_cast<lt::save_resume_data_failed_alert>(alert)) {
      fail_resume_request(implementation, *failed);
    }
    if (lt::alert_cast<lt::torrent_removed_alert>(alert) != nullptr &&
        torrent_id != 0) {
      completed_removals.push_back(torrent_id);
    }
    append_alert(implementation, *alert, torrent_id);
  }
  implementation.torrents.erase(
      std::remove_if(implementation.torrents.begin(),
                     implementation.torrents.end(),
                     [&completed_removals](const auto &record) {
                       return !record.active &&
                              std::find(completed_removals.begin(),
                                        completed_removals.end(),
                                        record.id) != completed_removals.end();
                     }),
      implementation.torrents.end());
}

template <typename Function>
rust::String mutate_torrent(SessionHandle &handle, std::uint64_t torrent_id,
                            std::string_view operation,
                            Function &&function) noexcept {
  try {
    auto &implementation = require_impl(handle);
    auto &record = find_torrent(implementation, torrent_id);
    function(implementation, record);
    return {};
  } catch (const std::exception &error) {
    return exception_message(operation, error);
  } catch (...) {
    return unknown_exception_message(operation);
  }
}

} // namespace

TorrentMutationFfi add_magnet(SessionHandle &handle, rust::String uri,
                              AddTorrentOptionsFfi options) noexcept {
  TorrentMutationFfi result{};
  try {
    const auto owned_uri = owned_string(uri);
    validate_string(owned_uri, max_magnet_uri_bytes, "magnet URI");
    lt::error_code error;
    auto parameters = lt::parse_magnet_uri(owned_uri, error);
    if (error) {
      throw std::runtime_error(error.message());
    }
    apply_add_options(parameters, options);
    return add_parameters(handle, std::move(parameters));
  } catch (const std::exception &error) {
    result.error = exception_message("adding magnet URI", error);
  } catch (...) {
    result.error = unknown_exception_message("adding magnet URI");
  }
  return result;
}

TorrentMutationFfi
add_torrent_metainfo(SessionHandle &handle,
                     rust::Vec<std::uint8_t> metainfo,
                     AddTorrentOptionsFfi options) noexcept {
  TorrentMutationFfi result{};
  try {
    validate_bytes(metainfo.size(), max_torrent_bytes, "torrent metainfo");
    auto parameters = lt::load_torrent_buffer(
        byte_span(metainfo), loading_limits(max_torrent_bytes));
    if (!parameters.ti) {
      throw std::runtime_error("torrent metainfo has no info dictionary");
    }
    if (parameters.ti->num_files() < 0 ||
        static_cast<std::size_t>(parameters.ti->num_files()) >
            max_file_count) {
      throw std::invalid_argument("torrent metainfo exceeds the file limit");
    }
    apply_add_options(parameters, options);
    return add_parameters(handle, std::move(parameters));
  } catch (const std::exception &error) {
    result.error = exception_message("adding torrent metainfo", error);
  } catch (...) {
    result.error = unknown_exception_message("adding torrent metainfo");
  }
  return result;
}

TorrentMutationFfi
add_resume_data(SessionHandle &handle,
                rust::Vec<std::uint8_t> resume_data,
                AddTorrentOptionsFfi options) noexcept {
  TorrentMutationFfi result{};
  try {
    validate_bytes(resume_data.size(), max_resume_bytes, "resume data");
    lt::error_code error;
    auto parameters = lt::read_resume_data(
        byte_span(resume_data), error, loading_limits(max_resume_bytes));
    if (error) {
      throw std::runtime_error(error.message());
    }
    apply_add_options(parameters, options);
    return add_parameters(handle, std::move(parameters));
  } catch (const std::exception &error) {
    result.error = exception_message("restoring resume data", error);
  } catch (...) {
    result.error = unknown_exception_message("restoring resume data");
  }
  return result;
}

rust::String pause_torrent(SessionHandle &handle,
                           std::uint64_t torrent_id) noexcept {
  return mutate_torrent(
      handle, torrent_id, "pausing torrent",
      [](auto &, const auto &record) { record.handle.pause(); });
}

rust::String resume_torrent(SessionHandle &handle,
                            std::uint64_t torrent_id) noexcept {
  return mutate_torrent(
      handle, torrent_id, "resuming torrent",
      [](auto &, const auto &record) { record.handle.resume(); });
}

rust::String remove_torrent(SessionHandle &handle,
                            std::uint64_t torrent_id) noexcept {
  return mutate_torrent(
      handle, torrent_id, "removing torrent", [](auto &implementation,
                                                 auto &record) {
        implementation.session->remove_torrent(record.handle);
        record.active = false;
      });
}

rust::String set_file_priority(SessionHandle &handle,
                               std::uint64_t torrent_id,
                               std::uint32_t file_index,
                               std::uint8_t priority) noexcept {
  return mutate_torrent(
      handle, torrent_id, "setting file priority",
      [file_index, priority](auto &, const auto &record) {
        validate_priority(priority);
        const auto torrent_info = record.handle.torrent_file();
        if (!torrent_info) {
          throw std::runtime_error("torrent metadata is not available");
        }
        if (file_index >=
            static_cast<std::uint32_t>(torrent_info->num_files())) {
          throw std::out_of_range("file index is out of range");
        }
        record.handle.file_priority(
            lt::file_index_t{static_cast<int>(file_index)},
            lt::download_priority_t{priority});
      });
}

rust::String set_file_priorities(SessionHandle &handle,
                                 std::uint64_t torrent_id,
                                 rust::Vec<std::uint8_t> priorities) noexcept {
  return mutate_torrent(
      handle, torrent_id, "setting file priorities",
      [&priorities](auto &, const auto &record) {
        const auto torrent_info = record.handle.torrent_file();
        if (!torrent_info) {
          throw std::runtime_error("torrent metadata is not available");
        }
        if (priorities.size() !=
            static_cast<std::size_t>(torrent_info->num_files())) {
          throw std::invalid_argument(
              "file priority count must match the torrent file count");
        }
        std::vector<lt::download_priority_t> native_priorities;
        native_priorities.reserve(priorities.size());
        for (const auto priority : priorities) {
          validate_priority(priority);
          native_priorities.emplace_back(lt::download_priority_t{priority});
        }
        record.handle.prioritize_files(native_priorities);
      });
}

rust::String force_recheck(SessionHandle &handle,
                           std::uint64_t torrent_id) noexcept {
  return mutate_torrent(
      handle, torrent_id, "forcing torrent recheck",
      [](auto &, const auto &record) { record.handle.force_recheck(); });
}

rust::String force_reannounce(SessionHandle &handle,
                              std::uint64_t torrent_id) noexcept {
  return mutate_torrent(
      handle, torrent_id, "forcing torrent reannounce",
      [](auto &, const auto &record) {
        record.handle.force_reannounce(
            0, -1, lt::torrent_handle::ignore_min_interval |
                       lt::torrent_handle::high_priority);
      });
}

rust::String set_torrent_limits(SessionHandle &handle,
                                std::uint64_t torrent_id,
                                std::int32_t download_limit,
                                std::int32_t upload_limit) noexcept {
  return mutate_torrent(
      handle, torrent_id, "setting torrent rate limits",
      [download_limit, upload_limit](auto &, const auto &record) {
        validate_rate_limit(download_limit);
        validate_rate_limit(upload_limit);
        record.handle.set_download_limit(download_limit);
        record.handle.set_upload_limit(upload_limit);
      });
}

rust::String set_global_limits(SessionHandle &handle,
                               std::int32_t download_limit,
                               std::int32_t upload_limit) noexcept {
  try {
    validate_rate_limit(download_limit);
    validate_rate_limit(upload_limit);
    auto &implementation = require_impl(handle);
    lt::settings_pack settings;
    settings.set_int(lt::settings_pack::download_rate_limit,
                     download_limit == -1 ? 0 : download_limit);
    settings.set_int(lt::settings_pack::upload_rate_limit,
                     upload_limit == -1 ? 0 : upload_limit);
    implementation.session->apply_settings(std::move(settings));
    return {};
  } catch (const std::exception &error) {
    return exception_message("setting global rate limits", error);
  } catch (...) {
    return unknown_exception_message("setting global rate limits");
  }
}

ResumeRequestFfi save_resume_data(SessionHandle &handle,
                                  std::uint64_t torrent_id) noexcept {
  ResumeRequestFfi result{};
  try {
    auto &implementation = require_impl(handle);
    auto &record = find_torrent(implementation, torrent_id);
    const auto request_id = implementation.next_resume_request_id++;
    implementation.resume_requests.push_back(
        {request_id, record.handle, 0, {}, {}});
    try {
      record.handle.save_resume_data(lt::torrent_handle::save_info_dict);
    } catch (...) {
      implementation.resume_requests.pop_back();
      throw;
    }
    result.request_id = request_id;
  } catch (const std::exception &error) {
    result.error = exception_message("requesting resume data", error);
  } catch (...) {
    result.error = unknown_exception_message("requesting resume data");
  }
  return result;
}

ResumeDataFfi poll_resume_data(SessionHandle &handle,
                               std::uint64_t request_id) noexcept {
  ResumeDataFfi result{};
  try {
    auto &implementation = require_impl(handle);
    process_alerts(implementation);
    const auto request = std::find_if(
        implementation.resume_requests.begin(),
        implementation.resume_requests.end(),
        [request_id](const auto &candidate) {
          return candidate.id == request_id;
        });
    if (request == implementation.resume_requests.end()) {
      throw std::invalid_argument("unknown resume-data request ID");
    }
    result.state = request->state;
    if (request->state == 0) {
      return result;
    }
    if (request->state == 1) {
      result.bytes.reserve(request->bytes.size());
      for (const auto byte : request->bytes) {
        result.bytes.push_back(byte);
      }
    } else {
      result.error = rust_string(request->error);
    }
    implementation.resume_requests.erase(request);
  } catch (const std::exception &error) {
    result.state = 2;
    result.error = exception_message("polling resume data", error);
  } catch (...) {
    result.state = 2;
    result.error = unknown_exception_message("polling resume data");
  }
  return result;
}

SessionSnapshotFfi snapshot_session(SessionHandle &handle) noexcept {
  SessionSnapshotFfi result{};
  try {
    auto &implementation = require_impl(handle);
    process_alerts(implementation);
    auto &session = *implementation.session;
    result.is_paused = session.is_paused();
    result.is_listening = session.is_listening();
    result.listen_port = session.listen_port();
    const auto settings = session.get_settings();
    const auto global_download =
        settings.get_int(lt::settings_pack::download_rate_limit);
    const auto global_upload =
        settings.get_int(lt::settings_pack::upload_rate_limit);
    result.global_download_limit =
        global_download == 0 ? -1 : global_download;
    result.global_upload_limit = global_upload == 0 ? -1 : global_upload;
    for (const auto &record : implementation.torrents) {
      if (record.active) {
        result.torrents.emplace_back(snapshot_torrent(record));
      }
    }
    result.torrent_count =
        static_cast<std::uint64_t>(result.torrents.size());
    result.alerts.reserve(implementation.alert_backlog.size());
    for (auto &alert : implementation.alert_backlog) {
      result.alerts.emplace_back(std::move(alert));
    }
    implementation.alert_backlog.clear();
  } catch (const std::exception &error) {
    result.error =
        exception_message("reading libtorrent session snapshot", error);
  } catch (...) {
    result.error =
        unknown_exception_message("reading libtorrent session snapshot");
  }
  return result;
}

rust::String shutdown_session(SessionHandle &handle) noexcept {
  try {
    if (handle.impl()) {
      stop_session(handle.impl()->session);
    }
    return {};
  } catch (const std::exception &error) {
    return exception_message("shutting down libtorrent session", error);
  } catch (...) {
    return unknown_exception_message("shutting down libtorrent session");
  }
}

} // namespace pubky_swarm::libtorrent
