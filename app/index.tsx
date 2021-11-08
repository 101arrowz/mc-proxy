import { useState } from 'react';
import { render } from 'react-dom';
import { begin } from './util/ipc'


const App = () => {
    const [username, setUsername] = useState('');
    const [password, setPassword] = useState('');
    const [apiKey, setApiKey] = useState('');
    return (<>
        <input placeholder="Username" onChange={e => setUsername(e.currentTarget.value)} value={username} />
        <input type="password" placeholder="Password" onChange={e => setPassword(e.currentTarget.value)} value={password} />
        <input placeholder="API Key" onChange={e => setApiKey(e.currentTarget.value)} value={apiKey} />
        <button onClick={() => begin({ username: username || undefined, password: password || undefined, apiKey: apiKey || undefined })}>Start</button>
    </>);
};

render(<App />, document.getElementById('root'));