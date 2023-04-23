a simple http server forward request start with `/openai/` to openai.  

use `openai_proxy_port` env variable to set port (default is 4000).    
use `HTTP_PROXY` or `HTTPS_PROXY` to use proxy.


--- 

add request and body output.  
If you want to see the request and response body of some frameworks(like langchain for example), you can just add one line to your python code (before import related packages).  
```python
os.environ["OPENAI_API_BASE"] = "http://localhost:4000/openai/v1"

from langchain.llms import OpenAI
import langchain ... ...
```