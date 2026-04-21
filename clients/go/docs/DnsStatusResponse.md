# DnsStatusResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BindAddr** | Pointer to **NullableString** | DNS server bind address | [optional] 
**Enabled** | **bool** | Whether DNS service is enabled | 
**Port** | Pointer to **NullableInt32** | DNS server port | [optional] 
**ServiceCount** | **int32** | Number of registered services | 
**Services** | **[]string** | List of registered service names | 
**Zone** | Pointer to **NullableString** | DNS zone name | [optional] 

## Methods

### NewDnsStatusResponse

`func NewDnsStatusResponse(enabled bool, serviceCount int32, services []string, ) *DnsStatusResponse`

NewDnsStatusResponse instantiates a new DnsStatusResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDnsStatusResponseWithDefaults

`func NewDnsStatusResponseWithDefaults() *DnsStatusResponse`

NewDnsStatusResponseWithDefaults instantiates a new DnsStatusResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBindAddr

`func (o *DnsStatusResponse) GetBindAddr() string`

GetBindAddr returns the BindAddr field if non-nil, zero value otherwise.

### GetBindAddrOk

`func (o *DnsStatusResponse) GetBindAddrOk() (*string, bool)`

GetBindAddrOk returns a tuple with the BindAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBindAddr

`func (o *DnsStatusResponse) SetBindAddr(v string)`

SetBindAddr sets BindAddr field to given value.

### HasBindAddr

`func (o *DnsStatusResponse) HasBindAddr() bool`

HasBindAddr returns a boolean if a field has been set.

### SetBindAddrNil

`func (o *DnsStatusResponse) SetBindAddrNil(b bool)`

 SetBindAddrNil sets the value for BindAddr to be an explicit nil

### UnsetBindAddr
`func (o *DnsStatusResponse) UnsetBindAddr()`

UnsetBindAddr ensures that no value is present for BindAddr, not even an explicit nil
### GetEnabled

`func (o *DnsStatusResponse) GetEnabled() bool`

GetEnabled returns the Enabled field if non-nil, zero value otherwise.

### GetEnabledOk

`func (o *DnsStatusResponse) GetEnabledOk() (*bool, bool)`

GetEnabledOk returns a tuple with the Enabled field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnabled

`func (o *DnsStatusResponse) SetEnabled(v bool)`

SetEnabled sets Enabled field to given value.


### GetPort

`func (o *DnsStatusResponse) GetPort() int32`

GetPort returns the Port field if non-nil, zero value otherwise.

### GetPortOk

`func (o *DnsStatusResponse) GetPortOk() (*int32, bool)`

GetPortOk returns a tuple with the Port field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPort

`func (o *DnsStatusResponse) SetPort(v int32)`

SetPort sets Port field to given value.

### HasPort

`func (o *DnsStatusResponse) HasPort() bool`

HasPort returns a boolean if a field has been set.

### SetPortNil

`func (o *DnsStatusResponse) SetPortNil(b bool)`

 SetPortNil sets the value for Port to be an explicit nil

### UnsetPort
`func (o *DnsStatusResponse) UnsetPort()`

UnsetPort ensures that no value is present for Port, not even an explicit nil
### GetServiceCount

`func (o *DnsStatusResponse) GetServiceCount() int32`

GetServiceCount returns the ServiceCount field if non-nil, zero value otherwise.

### GetServiceCountOk

`func (o *DnsStatusResponse) GetServiceCountOk() (*int32, bool)`

GetServiceCountOk returns a tuple with the ServiceCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServiceCount

`func (o *DnsStatusResponse) SetServiceCount(v int32)`

SetServiceCount sets ServiceCount field to given value.


### GetServices

`func (o *DnsStatusResponse) GetServices() []string`

GetServices returns the Services field if non-nil, zero value otherwise.

### GetServicesOk

`func (o *DnsStatusResponse) GetServicesOk() (*[]string, bool)`

GetServicesOk returns a tuple with the Services field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetServices

`func (o *DnsStatusResponse) SetServices(v []string)`

SetServices sets Services field to given value.


### GetZone

`func (o *DnsStatusResponse) GetZone() string`

GetZone returns the Zone field if non-nil, zero value otherwise.

### GetZoneOk

`func (o *DnsStatusResponse) GetZoneOk() (*string, bool)`

GetZoneOk returns a tuple with the Zone field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetZone

`func (o *DnsStatusResponse) SetZone(v string)`

SetZone sets Zone field to given value.

### HasZone

`func (o *DnsStatusResponse) HasZone() bool`

HasZone returns a boolean if a field has been set.

### SetZoneNil

`func (o *DnsStatusResponse) SetZoneNil(b bool)`

 SetZoneNil sets the value for Zone to be an explicit nil

### UnsetZone
`func (o *DnsStatusResponse) UnsetZone()`

UnsetZone ensures that no value is present for Zone, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


