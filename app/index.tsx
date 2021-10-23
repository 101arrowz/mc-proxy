import { render } from 'react-dom';
import { begin } from './util/ipc'

render(<button onClick={() => {
    begin();
}}>Hello sworld</button>, document.getElementById('root'));