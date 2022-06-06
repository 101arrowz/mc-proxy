import { useState } from 'react';
import { render } from 'react-dom';
import { begin } from './util/ipc'
import { WebviewWindow } from '@tauri-apps/api/window';

const App = () => {
    const [username, setUsername] = useState('');
    const [password, setPassword] = useState('');
    const [apiKey, setApiKey] = useState('');
    return (<>
        <input placeholder="Username" onChange={e => setUsername(e.currentTarget.value)} value={username} />
        <input type="password" placeholder="Password" onChange={e => setPassword(e.currentTarget.value)} value={password} />
        <input placeholder="API Key" onChange={e => setApiKey(e.currentTarget.value)} value={apiKey} />
        <button onClick={() => begin({ username: username || undefined, password: password || undefined, apiKey: apiKey || undefined })}>Start</button>
        <button onClick={() => {
            const msftWindow = new WebviewWindow('ms-oauth2', {
                url: 'https://login.live.com/oauth20_authorize.srf?client_id=128cac2a-5362-4fa5-ade3-267ed3c12503&response_type=token&redirect_uri=https%3A%2F%2Flogin.microsoftonline.com%2Fcommon%2Foauth2%2Fnativeclient&scope=XboxLive.signin'
            });
            msftWindow.show();
        }}>Login with Microsoft</button>
    </>);
};

render(<App />, document.getElementById('root'));