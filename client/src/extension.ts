import * as path from 'path';
import { ExtensionContext, workspace, window } from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

export async function activate(context: ExtensionContext): Promise<void> {
    const serverBinary =
        process.env.URDF_LSP_BIN ??
        (() => {
            const release = context.asAbsolutePath(
                path.join('server', 'bin', 'urdf-lsp'),
            );
            const debug = context.asAbsolutePath(
                path.join('server', 'target', 'debug', 'urdf-lsp'),
            );
            const fs = require('fs') as typeof import('fs');
            return fs.existsSync(release) ? release : debug;
        })();

    const serverOptions: ServerOptions = {
        command: serverBinary,
        args: [],
        transport: TransportKind.stdio,
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'urdf' }],
        synchronize: {
            fileEvents: workspace.createFileSystemWatcher('**/*.{urdf,xacro}'),
        },
    };

    client = new LanguageClient(
        'urdfLsp',
        'URDF Language Server',
        serverOptions,
        clientOptions,
    );

    try {
        await client.start();
    } catch (err) {
        window.showErrorMessage(
            `Failed to start urdf-lsp at ${serverBinary}: ${err instanceof Error ? err.message : String(err)}`,
        );
    }
}

export function deactivate(): Thenable<void> | undefined {
    return client?.stop();
}
