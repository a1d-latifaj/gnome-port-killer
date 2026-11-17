import Gio from 'gi://Gio';
const settings = new Gio.Settings({schema_id: 'org.gnome.shell.extensions.port-killer'});
if (ARGV[0] === 'write') {
    settings.set_boolean('notify-enabled', false);
    settings.set_string('protocol-filter', 'UDP');
    settings.set_string('view-mode', 'ALL');
    settings.set_strv('dev-includes', ['TCP:18000:custom-api']);
    settings.set_strv('dev-excludes', ['TCP:8000:node']);
    Gio.Settings.sync();
} else {
    if (settings.get_boolean('notify-enabled') !== false || settings.get_string('protocol-filter') !== 'UDP')
        throw new Error('Preferences did not survive recreation in a separate process');
    if (settings.get_string('view-mode') !== 'ALL' ||
        settings.get_strv('dev-includes')[0] !== 'TCP:18000:custom-api' ||
        settings.get_strv('dev-excludes')[0] !== 'TCP:8000:node') throw new Error('Developer preferences not persisted');
    print('PASS: notification and protocol preferences persisted across processes');
}
