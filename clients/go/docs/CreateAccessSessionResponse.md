# CreateAccessSessionResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Endpoint** | **string** | Original endpoint requested. | 
**ExpiresAt** | **int64** | Unix timestamp at which the session expires. | 
**LocalAddr** | **string** | Local address the daemon bound for this session, e.g. &#x60;127.0.0.1:30042&#x60;. | 
**SessionId** | **string** | Unique session identifier (UUID). | 

## Methods

### NewCreateAccessSessionResponse

`func NewCreateAccessSessionResponse(endpoint string, expiresAt int64, localAddr string, sessionId string, ) *CreateAccessSessionResponse`

NewCreateAccessSessionResponse instantiates a new CreateAccessSessionResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateAccessSessionResponseWithDefaults

`func NewCreateAccessSessionResponseWithDefaults() *CreateAccessSessionResponse`

NewCreateAccessSessionResponseWithDefaults instantiates a new CreateAccessSessionResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetEndpoint

`func (o *CreateAccessSessionResponse) GetEndpoint() string`

GetEndpoint returns the Endpoint field if non-nil, zero value otherwise.

### GetEndpointOk

`func (o *CreateAccessSessionResponse) GetEndpointOk() (*string, bool)`

GetEndpointOk returns a tuple with the Endpoint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEndpoint

`func (o *CreateAccessSessionResponse) SetEndpoint(v string)`

SetEndpoint sets Endpoint field to given value.


### GetExpiresAt

`func (o *CreateAccessSessionResponse) GetExpiresAt() int64`

GetExpiresAt returns the ExpiresAt field if non-nil, zero value otherwise.

### GetExpiresAtOk

`func (o *CreateAccessSessionResponse) GetExpiresAtOk() (*int64, bool)`

GetExpiresAtOk returns a tuple with the ExpiresAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExpiresAt

`func (o *CreateAccessSessionResponse) SetExpiresAt(v int64)`

SetExpiresAt sets ExpiresAt field to given value.


### GetLocalAddr

`func (o *CreateAccessSessionResponse) GetLocalAddr() string`

GetLocalAddr returns the LocalAddr field if non-nil, zero value otherwise.

### GetLocalAddrOk

`func (o *CreateAccessSessionResponse) GetLocalAddrOk() (*string, bool)`

GetLocalAddrOk returns a tuple with the LocalAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLocalAddr

`func (o *CreateAccessSessionResponse) SetLocalAddr(v string)`

SetLocalAddr sets LocalAddr field to given value.


### GetSessionId

`func (o *CreateAccessSessionResponse) GetSessionId() string`

GetSessionId returns the SessionId field if non-nil, zero value otherwise.

### GetSessionIdOk

`func (o *CreateAccessSessionResponse) GetSessionIdOk() (*string, bool)`

GetSessionIdOk returns a tuple with the SessionId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSessionId

`func (o *CreateAccessSessionResponse) SetSessionId(v string)`

SetSessionId sets SessionId field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


