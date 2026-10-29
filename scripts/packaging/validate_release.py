import importlib.util, os
spec = importlib.util.spec_from_file_location('release', 'scripts/packaging/release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
release.version_for_tag(os.environ['RELEASE_TAG'])
required = ['ALACRTERM_UPDATE_PUBLIC_KEYS', 'SPARKLE_PUBLIC_KEY', 'MINISIGN_SECRET_KEY', 'SPARKLE_PRIVATE_KEY', 'APPLE_CERTIFICATE_P12', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY', 'APPLE_API_KEY_P8', 'APPLE_API_KEY_ID', 'APPLE_API_ISSUER']
missing = [name for name in required if not os.environ.get(name)]
if missing:
    raise SystemExit('Missing release configuration: ' + ', '.join(missing))
