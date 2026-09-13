# frozen_string_literal: true

require "digest"

archive, checksums = ARGV
abort("Usage: verify-release-archive.rb ARCHIVE SHA256SUMS") unless archive && checksums
name = File.basename(archive)
entries = File.readlines(checksums).map do |line|
  match = line.match(/\A([a-f0-9]{64}) [ *](.+)\r?\n?\z/)
  match[1] if match && match[2].strip == name
end.compact
abort("Expected exactly one checksum for #{name}") unless entries.length == 1
abort("Checksum mismatch for #{name}") unless Digest::SHA256.file(archive).hexdigest == entries.first
