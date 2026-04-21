# PeerInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**FailureCount** | **int32** | Number of consecutive health check failures | 
**Healthy** | **bool** | Whether the peer is healthy | 
**LastCheck** | **int64** | Last health check timestamp (unix epoch seconds) | 
**LastHandshakeSecs** | Pointer to **NullableInt64** | Seconds since last handshake | [optional] 
**LastPingMs** | Pointer to **NullableInt64** | Last ping latency in milliseconds | [optional] 
**OverlayIp** | Pointer to **NullableString** | Peer&#39;s overlay IP address | [optional] 
**PublicKey** | **string** | Peer&#39;s public key | 

## Methods

### NewPeerInfo

`func NewPeerInfo(failureCount int32, healthy bool, lastCheck int64, publicKey string, ) *PeerInfo`

NewPeerInfo instantiates a new PeerInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewPeerInfoWithDefaults

`func NewPeerInfoWithDefaults() *PeerInfo`

NewPeerInfoWithDefaults instantiates a new PeerInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetFailureCount

`func (o *PeerInfo) GetFailureCount() int32`

GetFailureCount returns the FailureCount field if non-nil, zero value otherwise.

### GetFailureCountOk

`func (o *PeerInfo) GetFailureCountOk() (*int32, bool)`

GetFailureCountOk returns a tuple with the FailureCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFailureCount

`func (o *PeerInfo) SetFailureCount(v int32)`

SetFailureCount sets FailureCount field to given value.


### GetHealthy

`func (o *PeerInfo) GetHealthy() bool`

GetHealthy returns the Healthy field if non-nil, zero value otherwise.

### GetHealthyOk

`func (o *PeerInfo) GetHealthyOk() (*bool, bool)`

GetHealthyOk returns a tuple with the Healthy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHealthy

`func (o *PeerInfo) SetHealthy(v bool)`

SetHealthy sets Healthy field to given value.


### GetLastCheck

`func (o *PeerInfo) GetLastCheck() int64`

GetLastCheck returns the LastCheck field if non-nil, zero value otherwise.

### GetLastCheckOk

`func (o *PeerInfo) GetLastCheckOk() (*int64, bool)`

GetLastCheckOk returns a tuple with the LastCheck field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastCheck

`func (o *PeerInfo) SetLastCheck(v int64)`

SetLastCheck sets LastCheck field to given value.


### GetLastHandshakeSecs

`func (o *PeerInfo) GetLastHandshakeSecs() int64`

GetLastHandshakeSecs returns the LastHandshakeSecs field if non-nil, zero value otherwise.

### GetLastHandshakeSecsOk

`func (o *PeerInfo) GetLastHandshakeSecsOk() (*int64, bool)`

GetLastHandshakeSecsOk returns a tuple with the LastHandshakeSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastHandshakeSecs

`func (o *PeerInfo) SetLastHandshakeSecs(v int64)`

SetLastHandshakeSecs sets LastHandshakeSecs field to given value.

### HasLastHandshakeSecs

`func (o *PeerInfo) HasLastHandshakeSecs() bool`

HasLastHandshakeSecs returns a boolean if a field has been set.

### SetLastHandshakeSecsNil

`func (o *PeerInfo) SetLastHandshakeSecsNil(b bool)`

 SetLastHandshakeSecsNil sets the value for LastHandshakeSecs to be an explicit nil

### UnsetLastHandshakeSecs
`func (o *PeerInfo) UnsetLastHandshakeSecs()`

UnsetLastHandshakeSecs ensures that no value is present for LastHandshakeSecs, not even an explicit nil
### GetLastPingMs

`func (o *PeerInfo) GetLastPingMs() int64`

GetLastPingMs returns the LastPingMs field if non-nil, zero value otherwise.

### GetLastPingMsOk

`func (o *PeerInfo) GetLastPingMsOk() (*int64, bool)`

GetLastPingMsOk returns a tuple with the LastPingMs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastPingMs

`func (o *PeerInfo) SetLastPingMs(v int64)`

SetLastPingMs sets LastPingMs field to given value.

### HasLastPingMs

`func (o *PeerInfo) HasLastPingMs() bool`

HasLastPingMs returns a boolean if a field has been set.

### SetLastPingMsNil

`func (o *PeerInfo) SetLastPingMsNil(b bool)`

 SetLastPingMsNil sets the value for LastPingMs to be an explicit nil

### UnsetLastPingMs
`func (o *PeerInfo) UnsetLastPingMs()`

UnsetLastPingMs ensures that no value is present for LastPingMs, not even an explicit nil
### GetOverlayIp

`func (o *PeerInfo) GetOverlayIp() string`

GetOverlayIp returns the OverlayIp field if non-nil, zero value otherwise.

### GetOverlayIpOk

`func (o *PeerInfo) GetOverlayIpOk() (*string, bool)`

GetOverlayIpOk returns a tuple with the OverlayIp field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOverlayIp

`func (o *PeerInfo) SetOverlayIp(v string)`

SetOverlayIp sets OverlayIp field to given value.

### HasOverlayIp

`func (o *PeerInfo) HasOverlayIp() bool`

HasOverlayIp returns a boolean if a field has been set.

### SetOverlayIpNil

`func (o *PeerInfo) SetOverlayIpNil(b bool)`

 SetOverlayIpNil sets the value for OverlayIp to be an explicit nil

### UnsetOverlayIp
`func (o *PeerInfo) UnsetOverlayIp()`

UnsetOverlayIp ensures that no value is present for OverlayIp, not even an explicit nil
### GetPublicKey

`func (o *PeerInfo) GetPublicKey() string`

GetPublicKey returns the PublicKey field if non-nil, zero value otherwise.

### GetPublicKeyOk

`func (o *PeerInfo) GetPublicKeyOk() (*string, bool)`

GetPublicKeyOk returns a tuple with the PublicKey field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPublicKey

`func (o *PeerInfo) SetPublicKey(v string)`

SetPublicKey sets PublicKey field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


